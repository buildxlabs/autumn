//! The command palette: navigation, slash modes, and search. See §5 of
//! ARCHITECTURE.md and issue #63.
//!
//! The search side keeps two kinds of rows: local matches, computed
//! synchronously from a title cache kept in `db`, and remote matches from
//! AniList. `CommandState` has no delegate, so rows are rebuilt from the query
//! on every render; `on_confirm` resolves against the rows on screen.

use std::{collections::HashSet, rc::Rc, time::Duration};

use db::{CatalogTitle, Db};
use domain::MediaKind;
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, SharedString, Styled, Task, TestSupportExt, Window,
    assets::IconName,
    component::{
        ActiveTheme,
        command::{Command, CommandItem, CommandState},
        dialog::Cancel,
    },
    div,
};
use metadata::{AniList, MediaSort, MediaSummary, PageRequest, SearchFilters};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};
use ui_state::{Route, actions::COMMAND_PALETTE, bridge};

use crate::shell::{icon, title};

/// Pause before asking AniList, so typing a word is one request.
const DEBOUNCE: Duration = Duration::from_millis(350);
/// Shorter queries match too many titles to be useful.
const MIN_CHARS: usize = 2;
/// Results shown in either section, and in the empty-state lists.
const RESULT_LIMIT: usize = 10;

/// Routes the palette offers with no query, in order. Every route, unlike
/// the rail, which shows only some.
const ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];

/// A failed remote search. `offline` is inferred from the failure: the app
/// has no connectivity service yet, so a failed AniList request is the signal
/// to show local results only.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchError {
    /// What to log.
    pub(crate) message: String,
    /// Whether this failure means AniList is unreachable.
    pub(crate) offline: bool,
}

impl From<metadata::Error> for SearchError {
    fn from(error: metadata::Error) -> Self {
        let offline = matches!(error, metadata::Error::Net(_));
        Self {
            message: error.to_string(),
            offline,
        }
    }
}

impl From<bridge::BridgeError> for SearchError {
    fn from(error: bridge::BridgeError) -> Self {
        Self {
            message: error.to_string(),
            offline: true,
        }
    }
}

/// Where remote results come from. A trait so tests can count requests and
/// supply results without a network.
pub(crate) trait Catalog {
    /// Searches AniList; `kind` is `None` for `/search`, which covers both.
    fn search(
        &self,
        text: &str,
        kind: Option<MediaKind>,
        cx: &App,
    ) -> Task<Result<Vec<MediaSummary>, SearchError>>;
}

/// The public AniList catalog. Built here so `ui-screens` needs no `net` edge.
pub(crate) struct AniListCatalog {
    /// `None` when the shared HTTP client could not be built.
    client: Option<AniList>,
}

impl AniListCatalog {
    /// Builds the client once, at shell startup.
    pub(crate) fn new() -> Self {
        match AniList::with_default_client() {
            Ok(client) => Self {
                client: Some(client),
            },
            Err(error) => {
                tracing::warn!(%error, "AniList search is unavailable");
                Self { client: None }
            }
        }
    }
}

impl Catalog for AniListCatalog {
    fn search(
        &self,
        text: &str,
        kind: Option<MediaKind>,
        cx: &App,
    ) -> Task<Result<Vec<MediaSummary>, SearchError>> {
        let Some(client) = self.client.clone() else {
            return Task::ready(Err(SearchError {
                message: "no AniList client".to_owned(),
                offline: true,
            }));
        };
        let filters = SearchFilters {
            search: Some(text.to_owned()),
            kind,
            // Without this AniList returns results in id order.
            sort: Some(MediaSort::SearchMatch),
            ..SearchFilters::default()
        };
        let task = bridge::run(cx, async move {
            client
                .search(
                    &filters,
                    PageRequest::new(1, u32::try_from(RESULT_LIMIT).unwrap_or(u32::MAX)),
                )
                .await
        });
        cx.spawn(|_: &mut gpui_kit::AsyncApp| async move {
            match task.await {
                Ok(Ok(page)) => Ok(page.media),
                Ok(Err(error)) => Err(SearchError::from(error)),
                Err(error) => Err(SearchError::from(error)),
            }
        })
    }
}

/// What the palette lists, chosen by a slash command at the start of the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// No slash command: routes, or the slash commands once `/` is typed.
    Navigate,
    /// `/search`: anime and manga together.
    Search,
    /// `/anime`.
    Anime,
    /// `/manga`.
    Manga,
}

impl Mode {
    /// The modes a slash command selects, in the order they are listed.
    const SLASH: [Self; 3] = [Self::Search, Self::Anime, Self::Manga];

    /// The slash command that selects this mode; empty for [`Self::Navigate`].
    fn command(self) -> &'static str {
        match self {
            Self::Navigate => "",
            Self::Search => "/search",
            Self::Anime => "/anime",
            Self::Manga => "/manga",
        }
    }

    /// The query that opens the palette in this mode, ready for typing.
    fn prefix(self) -> SharedString {
        match self {
            Self::Navigate => SharedString::default(),
            Self::Search | Self::Anime | Self::Manga => format!("{} ", self.command()).into(),
        }
    }

    /// Splits a query into its mode and the text searched in that mode.
    ///
    /// A slash command counts only as a whole first word, so `/animes` stays
    /// a navigation query.
    fn parse(query: &str) -> (Self, &str) {
        let (word, rest) = query.split_once(' ').unwrap_or((query, ""));
        Self::SLASH
            .into_iter()
            .find(|mode| mode.command() == word)
            .map_or((Self::Navigate, query), |mode| (mode, rest.trim_start()))
    }

    /// Whether this mode shows `kind` media.
    fn includes(self, kind: MediaKind) -> bool {
        match self {
            Self::Navigate => false,
            Self::Search => true,
            Self::Anime => kind == MediaKind::Anime,
            Self::Manga => kind == MediaKind::Manga,
        }
    }

    /// The catalog filter for this mode: `None` means both kinds.
    fn kind(self) -> Option<MediaKind> {
        match self {
            Self::Anime => Some(MediaKind::Anime),
            Self::Manga => Some(MediaKind::Manga),
            Self::Search | Self::Navigate => None,
        }
    }
}

/// One search result row.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hit {
    /// A media from the local list or from AniList.
    Media {
        anilist_id: u32,
        title: String,
        kind: MediaKind,
        /// Whether it came from the local list (and so sorts above remote).
        local: bool,
    },
    /// A recent query, which fills the input when picked.
    Query(String),
}

impl Hit {
    fn anilist_id(&self) -> Option<u32> {
        match self {
            Self::Media { anilist_id, .. } => Some(*anilist_id),
            Self::Query(_) => None,
        }
    }

    fn label(&self) -> String {
        match self {
            Self::Media {
                title, local: true, ..
            } => format!("{title} · in your list"),
            Self::Media { title, .. } => title.clone(),
            Self::Query(query) => format!("Recent: {query}"),
        }
    }

    fn item(&self) -> CommandItem {
        let icon = match self {
            Self::Media { local: true, .. } => IconName::BookOpen,
            Self::Media { .. } | Self::Query(_) => IconName::Search,
        };
        CommandItem::new().label(self.label()).icon(icon)
    }
}

/// One row of the palette.
#[derive(Debug, Clone)]
enum Row {
    /// Navigates to the route.
    Route(Route),
    /// Switches to the mode.
    Mode(Mode),
    /// A search result.
    Hit(Hit),
}

impl Row {
    fn item(&self) -> CommandItem {
        match self {
            Self::Route(route) => CommandItem::new().label(title(*route)).icon(icon(*route)),
            Self::Mode(mode) => CommandItem::new()
                .label(mode.command())
                .icon(IconName::Search),
            Self::Hit(hit) => hit.item(),
        }
    }

    #[cfg(test)]
    fn label(&self) -> String {
        match self {
            Self::Route(route) => title(*route).to_string(),
            Self::Mode(mode) => mode.command().to_owned(),
            Self::Hit(hit) => hit.label(),
        }
    }
}

/// What the palette asks its host to do.
#[derive(Debug, Clone)]
pub(crate) enum PaletteEvent {
    /// The user picked a route; the host navigates and closes the palette.
    Navigate(Route),
    /// The user picked a media; the host records the view and closes the
    /// palette. Navigation to its media page is a later ticket.
    Viewed {
        /// The AniList id of the media.
        anilist_id: u32,
        /// Anime or manga.
        kind: MediaKind,
        /// The title to cache before recording the view.
        title: String,
    },
}

/// The local titles and recents loaded from `db` once per opening.
#[derive(Debug, Default)]
struct LocalIndex {
    titles: Vec<CatalogTitle>,
    searches: Vec<String>,
    views: Vec<CatalogTitle>,
}

/// The palette's view, kept alive between openings by the shell.
///
/// `Command` has no delegate, so the rows are rebuilt from the query on
/// every render.
pub(crate) struct CommandPalette {
    state: Entity<CommandState>,
    db: Db,
    catalog: Rc<dyn Catalog>,
    index: LocalIndex,
    /// Local matches for the current query, from `index.titles`.
    local: Vec<Hit>,
    /// Remote matches for `remote_for`.
    remote: Vec<Hit>,
    remote_for: String,
    /// Set when an AniList request fails, so only local rows are shown.
    offline: bool,
    index_task: Option<Task<()>>,
    debounce: Option<Task<()>>,
    search_task: Option<Task<()>>,
    write_task: Option<Task<Result<(), bridge::BridgeError>>>,
    view_task: Option<Task<Result<(), bridge::BridgeError>>>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    /// Builds a palette over `db` and `catalog`.
    pub(crate) fn new(
        window: &mut Window,
        cx: &mut Context<'_, Self>,
        db: Db,
        catalog: Rc<dyn Catalog>,
    ) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            db,
            catalog,
            index: LocalIndex::default(),
            local: Vec::new(),
            remote: Vec::new(),
            remote_for: String::new(),
            offline: false,
            index_task: None,
            debounce: None,
            search_task: None,
            write_task: None,
            view_task: None,
        }
    }

    /// Starts over in `mode` and focuses the query field. Every opening
    /// reloads the local side, so recents and newly cached titles are current.
    pub(crate) fn open(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.state.update(cx, |state, cx| {
            state.set_query(mode.prefix(), window, cx);
            state.focus(window, cx);
        });
        self.load(cx);
    }

    /// Reloads the local side from `db`.
    pub(crate) fn load(&mut self, cx: &mut Context<'_, Self>) {
        let db = self.db.clone();
        let task = bridge::run(cx, async move {
            let mut index = LocalIndex::default();
            for kind in [MediaKind::Anime, MediaKind::Manga] {
                match db.palette().local(kind).await {
                    Ok(titles) => index.titles.extend(titles),
                    Err(error) => tracing::warn!(?error, ?kind, "could not load palette titles"),
                }
                match db.palette().recent_views(kind).await {
                    Ok(views) => index.views.extend(views),
                    Err(error) => tracing::warn!(?error, ?kind, "could not load recent views"),
                }
            }
            match db.palette().recent_searches().await {
                Ok(searches) => index.searches = searches,
                Err(error) => tracing::warn!(?error, "could not load recent searches"),
            }
            index
        });
        self.index_task = Some(cx.spawn(async move |this, cx| {
            if let Ok(index) = task.await {
                let _ = this.update(cx, |palette, cx| {
                    palette.index = index;
                    palette.index_task = None;
                    palette.refresh(cx);
                });
            }
        }));
    }

    /// Recomputes the local rows and asks for a render.
    fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        let query = self.state.read(cx).query(cx);
        let (mode, text) = Mode::parse(&query);
        self.local = self.match_local(mode, text);
        cx.notify();
    }

    /// Fuzzy-matches cached titles, best first, capped at [`RESULT_LIMIT`].
    fn match_local(&self, mode: Mode, text: &str) -> Vec<Hit> {
        if text.is_empty() {
            return Vec::new();
        }
        let pattern = Pattern::new(
            text,
            CaseMatching::Smart,
            Normalization::Smart,
            AtomKind::Fuzzy,
        );
        if pattern.atoms.is_empty() {
            return Vec::new();
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let mut buf = Vec::new();
        let mut scored: Vec<(u32, &CatalogTitle)> = self
            .index
            .titles
            .iter()
            .filter(|candidate| mode.includes(candidate.kind))
            .filter_map(|candidate| {
                let mut best = {
                    let haystack = Utf32Str::new(&candidate.title, &mut buf);
                    pattern.score(haystack, &mut matcher)
                };
                for alias in &candidate.aliases {
                    let score = {
                        let haystack = Utf32Str::new(alias, &mut buf);
                        pattern.score(haystack, &mut matcher)
                    };
                    if score > best {
                        best = score;
                    }
                }
                best.map(|score| (score, candidate))
            })
            .collect();
        scored.sort_unstable_by_key(|(score, _)| std::cmp::Reverse(*score));
        scored
            .into_iter()
            .take(RESULT_LIMIT)
            .map(|(_, candidate)| Hit::Media {
                anilist_id: candidate.anilist_id,
                title: candidate.title.clone(),
                kind: candidate.kind,
                local: true,
            })
            .collect()
    }

    /// The rows for `mode`. Navigation is filtered locally by `Command`
    /// itself; search rows are already matched and deduplicated here.
    fn rows(&self, mode: Mode, query: &str) -> (Mode, Vec<Row>) {
        if mode == Mode::Navigate {
            let rows = if query.starts_with('/') {
                Mode::SLASH.into_iter().map(Row::Mode).collect()
            } else {
                ROUTES.into_iter().map(Row::Route).collect()
            };
            return (mode, rows);
        }
        let (_, text) = Mode::parse(query);
        let mut hits = if text.is_empty() {
            self.recent_hits(mode)
        } else {
            self.local.clone()
        };
        if !text.is_empty() && !self.offline && self.remote_for == text {
            hits.extend(self.remote.iter().cloned());
        }
        (mode, hits.into_iter().map(Row::Hit).collect())
    }

    /// Recent searches and views shown with no query.
    fn recent_hits(&self, mode: Mode) -> Vec<Hit> {
        let mut hits: Vec<Hit> = self
            .index
            .searches
            .iter()
            .take(RESULT_LIMIT / 2)
            .cloned()
            .map(Hit::Query)
            .collect();
        hits.extend(
            self.index
                .views
                .iter()
                .filter(|view| mode.includes(view.kind))
                .take(RESULT_LIMIT / 2)
                .map(|view| Hit::Media {
                    anilist_id: view.anilist_id,
                    title: view.title.clone(),
                    kind: view.kind,
                    local: false,
                }),
        );
        hits
    }

    /// A new query invalidates the old remote rows and restarts the timer.
    /// Dropping both tasks cancels a pending debounce and any request already
    /// in flight.
    fn query_changed(&mut self, query: &str, cx: &mut Context<'_, Self>) {
        let (mode, text) = Mode::parse(query);
        self.local = self.match_local(mode, text);
        self.remote.clear();
        self.remote_for.clear();
        self.debounce = None;
        self.search_task = None;
        if mode != Mode::Navigate && text.chars().count() >= MIN_CHARS {
            // A new attempt gets a fresh offline signal; a failure below
            // restores local-only rows for this query.
            self.offline = false;
            let text = text.to_owned();
            self.debounce = Some(cx.spawn(async move |this, cx| {
                cx.background_executor().timer(DEBOUNCE).await;
                let _ = this.update(cx, |palette, cx| palette.fetch(&text, mode, cx));
            }));
        }
        cx.notify();
    }

    /// Starts the one remote request for `text`.
    fn fetch(&mut self, text: &str, mode: Mode, cx: &mut Context<'_, Self>) {
        self.debounce = None;
        let task = self.catalog.search(text, mode.kind(), cx);
        let query = text.to_owned();
        self.search_task = Some(cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |palette, cx| palette.remote_done(query, result, cx));
        }));
    }

    /// Applies a remote result: cache it locally, deduplicate against the
    /// local rows, and cap the section at ten.
    fn remote_done(
        &mut self,
        query: String,
        result: Result<Vec<MediaSummary>, SearchError>,
        cx: &mut Context<'_, Self>,
    ) {
        self.search_task = None;
        match result {
            Ok(media) => {
                self.offline = false;
                self.remember(&query, &media, cx);
                // `remember` refreshed the local rows; dedup remote against
                // them and against earlier results on the same page.
                let mut seen: HashSet<u32> =
                    self.local.iter().filter_map(Hit::anilist_id).collect();
                self.remote = media
                    .into_iter()
                    .filter(|hit| seen.insert(hit.anilist_id))
                    .take(RESULT_LIMIT)
                    .map(|hit| Hit::Media {
                        anilist_id: hit.anilist_id,
                        title: display_title(&hit),
                        kind: hit.kind.unwrap_or(MediaKind::Anime),
                        local: false,
                    })
                    .collect();
                self.remote_for = query;
            }
            Err(error) => {
                self.offline = error.offline;
                self.remote.clear();
                tracing::warn!(message = %error.message, "palette search failed");
            }
        }
        cx.notify();
    }

    /// Records the query and caches the titles that arrived with the results.
    fn remember(&mut self, query: &str, media: &[MediaSummary], cx: &mut Context<'_, Self>) {
        let titles: Vec<CatalogTitle> = media.iter().filter_map(title_from).collect();
        let query = query.to_owned();
        let db = self.db.clone();
        self.write_task = Some(bridge::run(cx, async move {
            if let Err(error) = db.palette().record_search(&query).await {
                tracing::warn!(?error, "could not record a palette search");
            }
            if let Err(error) = db.palette().remember(&titles).await {
                tracing::warn!(?error, "could not cache palette titles");
            }
        }));
    }

    /// Caches the picked media's title and records it as recently viewed in
    /// one write: the view joins to the title, so neither can race the other.
    pub(crate) fn record_view(
        &mut self,
        anilist_id: u32,
        kind: MediaKind,
        title: String,
        cx: &mut Context<'_, Self>,
    ) {
        let db = self.db.clone();
        self.view_task = Some(bridge::run(cx, async move {
            let cached = [CatalogTitle {
                anilist_id,
                kind,
                title,
                aliases: Vec::new(),
            }];
            if let Err(error) = db.palette().remember(&cached).await {
                tracing::warn!(?error, "could not cache a palette view's title");
            }
            if let Err(error) = db.palette().record_view(anilist_id, kind).await {
                tracing::warn!(?error, "could not record a palette view");
            }
        }));
    }

    /// The query as typed.
    #[cfg(test)]
    pub(crate) fn query(&self, cx: &App) -> SharedString {
        self.state.read(cx).query(cx)
    }

    /// The mode the query selects.
    #[cfg(test)]
    pub(crate) fn mode(&self, cx: &App) -> Mode {
        Mode::parse(&self.query(cx)).0
    }

    /// The rows on screen, by label, for tests.
    #[cfg(test)]
    pub(crate) fn row_labels(&self, cx: &App) -> Vec<String> {
        let query = self.query(cx);
        let (_, rows) = self.rows(Mode::parse(&query).0, &query);
        rows.iter().map(Row::label).collect()
    }

    #[cfg(test)]
    pub(crate) fn offline(&self) -> bool {
        self.offline
    }

    #[cfg(test)]
    pub(crate) fn remote_for(&self) -> &str {
        &self.remote_for
    }

    #[cfg(test)]
    fn set_index(&mut self, index: LocalIndex, cx: &mut Context<'_, Self>) {
        self.index = index;
        self.refresh(cx);
    }

    fn confirm(&mut self, entry: Row, window: &mut Window, cx: &mut Context<'_, Self>) {
        match entry {
            Row::Route(route) => cx.emit(PaletteEvent::Navigate(route)),
            Row::Mode(mode) => self.open(mode, window, cx),
            Row::Hit(Hit::Query(query)) => {
                self.state.update(cx, |state, cx| {
                    state.set_query(format!("/search {query} "), window, cx);
                    state.focus(window, cx);
                });
            }
            Row::Hit(Hit::Media {
                anilist_id,
                kind,
                title,
                ..
            }) => {
                cx.emit(PaletteEvent::Viewed {
                    anilist_id,
                    kind,
                    title,
                });
            }
        }
    }

    /// Escape on a bare slash command, such as the `/search ` that `s`
    /// opens with, closes the palette instead of only clearing the query.
    ///
    /// It runs before the command list's own handler, which closes the
    /// dialog when the query is empty and clears it otherwise.
    fn cancel(&mut self, _: &Cancel, window: &mut Window, cx: &mut Context<'_, Self>) {
        let query = self.state.read(cx).query(cx);
        if let (Mode::Search | Mode::Anime | Mode::Manga, "") = Mode::parse(&query) {
            self.state
                .update(cx, |state, cx| state.set_query("", window, cx));
        }
    }
}

/// The title AniList's card shows, and the other titles as aliases.
fn title_from(summary: &MediaSummary) -> Option<CatalogTitle> {
    let kind = summary.kind?;
    let (title, romaji, native) = title_parts(summary)?;
    let aliases = [romaji, native]
        .into_iter()
        .flatten()
        .filter(|alias| *alias != title)
        .map(str::to_owned)
        .collect();
    Some(CatalogTitle {
        anilist_id: summary.anilist_id,
        kind,
        title: title.to_owned(),
        aliases,
    })
}

/// The best display title and the other two, in AniList's preference order.
fn title_parts(summary: &MediaSummary) -> Option<(&str, Option<&str>, Option<&str>)> {
    let romaji = summary.title.romaji.as_deref();
    let native = summary.title.native.as_deref();
    let title = summary.title.english.as_deref().or(romaji).or(native)?;
    Some((title, romaji, native))
}

fn display_title(summary: &MediaSummary) -> String {
    title_parts(summary)
        .map_or("Untitled", |(title, _, _)| title)
        .to_owned()
}

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let query = self.state.read(cx).query(cx);
        let (mode, rows) = self.rows(Mode::parse(&query).0, &query);
        let on_query = cx.entity().downgrade();
        let on_confirm = cx.entity().downgrade();
        let searching = self.search_task.is_some() || self.debounce.is_some();
        let empty_message = match (mode, searching) {
            (Mode::Navigate, _) => "No page or mode matches",
            (_, true) => "Searching…",
            (Mode::Search, false) => "No recent searches or media",
            (Mode::Anime, false) => "No anime matches",
            (Mode::Manga, false) => "No manga matches",
        };
        // The rows on screen, so a pick never lands in a newer query's list.
        let picked = rows.clone();
        div()
            .key_context(COMMAND_PALETTE)
            .capture_action(cx.listener(Self::cancel))
            .child(
                Command::new(&self.state)
                    .bordered(false)
                    .items(rows.iter().map(Row::item))
                    // Navigation filters locally; search rows arrive matched.
                    .filterable(mode == Mode::Navigate)
                    .placeholder("Go to a page, or type / for search modes")
                    .empty(move |_, _, cx| empty_state(empty_message, cx))
                    .on_query(move |query, _, cx| {
                        // Rows depend on the query, so render them again.
                        if let Some(palette) = on_query.upgrade() {
                            palette.update(cx, |palette, cx| palette.query_changed(query, cx));
                        }
                    })
                    .on_confirm(move |index, window, cx| {
                        let entry = picked.get(index.row).cloned();
                        if let (Some(entry), Some(palette)) = (entry, on_confirm.upgrade()) {
                            palette.update(cx, |palette, cx| palette.confirm(entry, window, cx));
                        }
                    }),
            )
    }
}

fn empty_state(message: &'static str, cx: &App) -> impl IntoElement + use<> {
    div()
        .id("palette-empty")
        .py_6()
        .text_center()
        .text_sm()
        .text_color(cx.theme().muted_foreground)
        .child(message)
        .test_support()
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use db::{CatalogTitle, Db};
    use domain::MediaKind;
    use gpui_kit::{
        App, AppContext, Bounds, Entity, TestAppContext, Window, WindowBounds, WindowHandle,
        WindowOptions, base::Root, point, px, size, test::TestWindowExt,
    };
    use metadata::{MediaSummary, MediaTitle};
    use tokio::runtime::Runtime;

    use super::{Catalog, CommandPalette, LocalIndex, Mode, SearchError};

    /// A catalog that records queries and answers from a table. Can be told
    /// to fail (offline) or to hold a request open (in flight).
    #[derive(Clone, Default)]
    struct FakeCatalog {
        requests: Arc<std::sync::Mutex<Vec<String>>>,
        results: Arc<std::sync::Mutex<BTreeMap<String, Vec<MediaSummary>>>>,
        offline: Arc<AtomicBool>,
        hang: Arc<AtomicBool>,
        live: Arc<AtomicUsize>,
    }

    impl FakeCatalog {
        fn result(&self, query: &str, media: Vec<MediaSummary>) {
            self.results.lock().unwrap().insert(query.to_owned(), media);
        }

        fn queries(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    /// Decrements the live count when a held request is dropped.
    struct Live(Arc<AtomicUsize>);

    impl Drop for Live {
        fn drop(&mut self) {
            self.0.fetch_sub(1, Ordering::SeqCst);
        }
    }

    impl Catalog for FakeCatalog {
        fn search(
            &self,
            text: &str,
            _kind: Option<MediaKind>,
            cx: &App,
        ) -> gpui_kit::Task<Result<Vec<MediaSummary>, SearchError>> {
            self.requests.lock().unwrap().push(text.to_owned());
            if self.hang.load(Ordering::SeqCst) {
                self.live.fetch_add(1, Ordering::SeqCst);
                let live = Arc::clone(&self.live);
                return cx.spawn(|_: &mut gpui_kit::AsyncApp| async move {
                    let _guard = Live(live);
                    std::future::pending::<Result<Vec<MediaSummary>, SearchError>>().await
                });
            }
            if self.offline.load(Ordering::SeqCst) {
                return gpui_kit::Task::ready(Err(SearchError {
                    message: "no connection".to_owned(),
                    offline: true,
                }));
            }
            let media = self
                .results
                .lock()
                .unwrap()
                .get(text)
                .cloned()
                .unwrap_or_default();
            gpui_kit::Task::ready(Ok(media))
        }
    }

    fn summary(id: u32, name: &str) -> MediaSummary {
        MediaSummary {
            anilist_id: id,
            mal_id: None,
            kind: Some(MediaKind::Anime),
            title: MediaTitle {
                romaji: Some(name.to_owned()),
                ..MediaTitle::default()
            },
            poster: None,
            banner: None,
            format: None,
            status: None,
            season: None,
            season_year: None,
            episodes: None,
            chapters: None,
            mean_score: None,
            genres: Vec::new(),
            is_adult: false,
        }
    }

    fn cached(id: u32, name: &str) -> CatalogTitle {
        CatalogTitle {
            anilist_id: id,
            kind: MediaKind::Anime,
            title: name.to_owned(),
            aliases: Vec::new(),
        }
    }

    struct Harness {
        // Keeps the runtime alive for the bridge's db writes.
        runtime: Runtime,
        window: WindowHandle<Root>,
        palette: Entity<CommandPalette>,
    }

    impl Harness {
        fn open(cx: &mut TestAppContext, catalog: Rc<dyn Catalog>) -> Self {
            // A current-thread runtime keeps Tokio wakeups on the test
            // thread, which the deterministic GPUI scheduler requires.
            let runtime = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .unwrap();
            let dir = tempfile::tempdir().unwrap();
            let db = runtime
                .block_on(Db::open(&dir.path().join("autumn.db")))
                .unwrap();
            let (window, palette) = cx.update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                ui_state::actions::bind_keys(cx);
                ui_state::bridge::init(cx, runtime.handle().clone());
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1024.), px(768.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| CommandPalette::new(window, cx, db, catalog))
                })
                .unwrap()
            });
            let window = window.downcast::<Root>().unwrap();
            let harness = Harness {
                runtime,
                window,
                palette,
            }
            .with(cx, Mode::Anime);
            // `open` reloads from db; let that finish before tests seed an index.
            for _ in 0..50 {
                harness.flush(cx);
                if !harness
                    .palette
                    .read_with(cx, |palette, _| palette.index_task.is_some())
                {
                    break;
                }
            }
            harness
        }

        fn with(self, cx: &mut TestAppContext, mode: Mode) -> Self {
            self.act(cx, |window, cx| {
                self.palette
                    .update(cx, |palette, cx| palette.open(mode, window, cx));
            });
            self
        }

        fn act(&self, cx: &mut TestAppContext, f: impl FnOnce(&mut Window, &mut App)) {
            cx.update_window(self.window.into(), |_, window, cx| {
                window.render_frame(cx);
                f(window, cx);
                window.render_frame(cx);
            })
            .unwrap();
        }

        /// Types text without letting the test clock reach the debounce
        /// timer: `run_until_parked` would advance it for us.
        fn input(&self, cx: &mut TestAppContext, text: &str) {
            self.act(cx, |window, cx| window.input(text, cx));
            self.flush(cx);
        }

        /// Clears the query and types a whole one, for step-by-step typing.
        fn set_query(&self, cx: &mut TestAppContext, query: &str) {
            self.act(cx, |window, cx| window.press("escape", cx));
            self.input(cx, query);
        }

        /// Drains GPUI work, then lets Tokio finish db writes on this same
        /// thread before draining any GPUI tasks those writes woke.
        fn flush(&self, cx: &mut TestAppContext) {
            while cx.executor().tick() {}
            self.runtime.block_on(async {
                for _ in 0..8 {
                    tokio::task::yield_now().await;
                }
            });
            while cx.executor().tick() {}
        }

        fn labels(&self, cx: &mut TestAppContext) -> Vec<String> {
            self.palette.read_with(cx, CommandPalette::row_labels)
        }

        fn index(&self, cx: &mut TestAppContext, index: LocalIndex) {
            self.palette
                .update(cx, |palette, cx| palette.set_index(index, cx));
        }

        fn pass_debounce(&self, cx: &mut TestAppContext) {
            cx.executor().advance_clock(Duration::from_millis(400));
            self.flush(cx);
        }
    }

    #[gpui_kit::test]
    fn local_matches_come_first_and_remote_hits_are_deduplicated(cx: &mut TestAppContext) {
        let catalog = FakeCatalog::default();
        catalog.result(
            "frieren",
            vec![
                summary(154_587, "Frieren remote"),
                summary(1, "Cowboy Bebop"),
            ],
        );
        let palette = Harness::open(cx, Rc::new(catalog));
        palette.index(
            cx,
            LocalIndex {
                titles: vec![cached(154_587, "Frieren")],
                ..LocalIndex::default()
            },
        );

        palette.input(cx, "frieren");
        palette.pass_debounce(cx);

        let labels = palette.labels(cx);
        assert_eq!(labels.len(), 2, "got {labels:?}");
        assert!(
            labels
                .first()
                .is_some_and(|label| label.contains("Frieren · in your list")),
            "the local match is first: {labels:?}"
        );
        assert!(
            labels
                .get(1)
                .is_some_and(|label| label.contains("Cowboy Bebop")),
            "only the non-duplicate remote hit remains: {labels:?}"
        );
    }

    #[gpui_kit::test]
    fn typing_fast_fires_one_request_after_a_pause(cx: &mut TestAppContext) {
        let catalog = FakeCatalog::default();
        let palette = Harness::open(cx, Rc::new(catalog.clone()));

        palette.set_query(cx, "/anime fr");
        palette.set_query(cx, "/anime fri");
        palette.set_query(cx, "/anime frie");
        cx.executor().advance_clock(Duration::from_millis(100));
        palette.flush(cx);
        assert!(
            catalog.queries().is_empty(),
            "no request before the pause: {:?}",
            catalog.queries()
        );

        cx.executor().advance_clock(Duration::from_millis(300));
        palette.flush(cx);
        assert_eq!(
            catalog.queries(),
            ["frie"],
            "one request for the settled text"
        );
    }

    #[gpui_kit::test]
    fn a_short_query_does_not_search(cx: &mut TestAppContext) {
        let catalog = FakeCatalog::default();
        let palette = Harness::open(cx, Rc::new(catalog.clone()));

        palette.input(cx, "f");
        palette.pass_debounce(cx);

        assert!(catalog.queries().is_empty(), "one character is too short");
    }

    #[gpui_kit::test]
    fn a_newer_query_cancels_the_request_in_flight(cx: &mut TestAppContext) {
        let catalog = FakeCatalog::default();
        catalog.hang.store(true, Ordering::SeqCst);
        catalog.result("naruto", vec![summary(2, "Naruto")]);
        let palette = Harness::open(cx, Rc::new(catalog.clone()));

        palette.set_query(cx, "/anime frieren");
        palette.pass_debounce(cx);
        assert_eq!(catalog.queries(), ["frieren"]);
        assert_eq!(
            catalog.live.load(Ordering::SeqCst),
            1,
            "first request is held"
        );

        palette.set_query(cx, "/anime naruto");
        assert_eq!(
            catalog.live.load(Ordering::SeqCst),
            0,
            "the newer query dropped the older request"
        );
        catalog.hang.store(false, Ordering::SeqCst);
        palette.pass_debounce(cx);

        assert_eq!(catalog.queries(), ["frieren", "naruto"]);
        let labels = palette.labels(cx);
        assert!(
            labels.iter().any(|label| label.contains("Naruto")),
            "the newer result is shown: {labels:?}"
        );
        assert!(
            !labels.iter().any(|label| label.contains("Frieren")),
            "the cancelled result never arrived: {labels:?}"
        );
        assert_eq!(
            palette
                .palette
                .read_with(cx, |p, _| p.remote_for().to_owned()),
            "naruto"
        );
    }

    #[gpui_kit::test]
    fn a_failed_search_shows_only_local_results(cx: &mut TestAppContext) {
        let catalog = FakeCatalog::default();
        catalog.offline.store(true, Ordering::SeqCst);
        let palette = Harness::open(cx, Rc::new(catalog));
        palette.index(
            cx,
            LocalIndex {
                titles: vec![cached(154_587, "Frieren")],
                ..LocalIndex::default()
            },
        );

        palette.input(cx, "frieren");
        palette.pass_debounce(cx);

        let labels = palette.labels(cx);
        assert_eq!(labels.len(), 1, "only the local match: {labels:?}");
        assert!(
            labels
                .first()
                .is_some_and(|label| label.contains("Frieren · in your list"))
        );
        assert!(palette.palette.read_with(cx, |p, _| p.offline()));
    }

    #[gpui_kit::test]
    fn an_empty_query_lists_recent_searches_and_views(cx: &mut TestAppContext) {
        let palette = Harness::open(cx, Rc::new(FakeCatalog::default()));
        palette.index(
            cx,
            LocalIndex {
                searches: (0..10).map(|n| format!("query {n}")).collect(),
                views: vec![cached(1, "Cowboy Bebop")],
                ..LocalIndex::default()
            },
        );

        let labels = palette.labels(cx);
        assert_eq!(labels.len(), 6, "five recent searches and one view");
        assert_eq!(labels.first().map(String::as_str), Some("Recent: query 0"));
        assert_eq!(
            labels.get(5).map(String::as_str),
            Some("Cowboy Bebop"),
            "views are not crowded out"
        );
    }
}
