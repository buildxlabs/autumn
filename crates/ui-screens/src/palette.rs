//! The command palette: route navigation and slash modes. See ARCHITECTURE §5.
//!
//! Search modes show Local platform list entries first, then AniList results.
//! Rows are one ungrouped list, so a confirmed `IndexPath` is a row number.

use db::Db;
use domain::{ListStatus, MediaKind};
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
use metadata::{AniList, MediaSummary, PageRequest, SearchFilters};
use ui_state::{ListTab, Route, actions::COMMAND_PALETTE, bridge};

use crate::{
    palette_search::{
        DEBOUNCE, MAX_RESULTS, display_label, local_matches, local_summary, merge, needs_remote,
        remember,
    },
    shell::{icon, title},
};

/// Routes the palette offers with no query, in order. Every route, unlike
/// the rail, which shows only some.
const ROUTES: [Route; 6] = [
    Route::Home,
    Route::Discover,
    Route::Search(ui_state::SearchFilters::EMPTY),
    Route::Lists {
        status: ListTab::Status(ListStatus::Current),
    },
    Route::Schedule,
    Route::Manga,
];

/// What the palette lists, chosen by a slash command at the start of the query.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    /// No slash command: routes, the slash commands or recent searches.
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

    /// The media kinds this mode lists, or none for navigation.
    fn kinds(self) -> &'static [MediaKind] {
        match self {
            Self::Navigate => &[],
            Self::Anime => &[MediaKind::Anime],
            Self::Manga => &[MediaKind::Manga],
            Self::Search => &[MediaKind::Anime, MediaKind::Manga],
        }
    }

    /// The AniList media type for a single-kind mode.
    fn kind(self) -> Option<MediaKind> {
        match self {
            Self::Anime => Some(MediaKind::Anime),
            Self::Manga => Some(MediaKind::Manga),
            Self::Search | Self::Navigate => None,
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
}

/// One row of the palette.
#[derive(Debug, Clone)]
enum Entry {
    /// Navigates to the route.
    Route(Route),
    /// Switches to the mode.
    Mode(Mode),
    /// A local or AniList search result.
    Media(MediaSummary),
    /// A query used earlier, for the empty-query mode.
    Search(String),
}

impl Entry {
    fn item(self) -> CommandItem {
        match self {
            Self::Route(route) => CommandItem::new().label(title(&route)).icon(icon(&route)),
            Self::Mode(mode) => CommandItem::new()
                .label(mode.command())
                .icon(IconName::Search),
            Self::Media(media) => CommandItem::new()
                .label(display_label(&media))
                .icon(IconName::Search),
            Self::Search(text) => CommandItem::new()
                .label(format!("Recent: {text}"))
                .icon(IconName::Search),
        }
    }
}

/// The rows for the whole query: slash commands once it starts with `/`,
/// recent searches plus routes when it is empty, otherwise local matches
/// above remote results (#63).
fn entries_for(
    query: &str,
    local: &[MediaSummary],
    remote: Option<&[MediaSummary]>,
    history: &[String],
) -> (Mode, Vec<Entry>) {
    let (mode, text) = Mode::parse(query);
    let rows = match mode {
        Mode::Navigate if query.starts_with('/') => {
            Mode::SLASH.into_iter().map(Entry::Mode).collect()
        }
        Mode::Navigate => history
            .iter()
            .cloned()
            .map(Entry::Search)
            .chain(ROUTES.into_iter().map(Entry::Route))
            .collect(),
        Mode::Search | Mode::Anime | Mode::Manga => {
            let kinds = mode.kinds();
            let local: Vec<MediaSummary> = local
                .iter()
                .filter(|media| media.kind.is_some_and(|kind| kinds.contains(&kind)))
                .cloned()
                .collect();
            merge(
                local_matches(text, &local),
                remote.map(<[MediaSummary]>::to_vec),
            )
            .into_iter()
            .map(Entry::Media)
            .collect()
        }
    };
    (mode, rows)
}

/// What the palette asks its host to do.
#[derive(Debug, Clone)]
pub(crate) enum PaletteEvent {
    /// The user picked a route; the host navigates and closes the palette.
    Navigate(Route),
}

/// The palette's view, kept alive between openings by the shell.
///
/// `Command` has no delegate, so the rows are rebuilt from the query on
/// every render.
pub(crate) struct CommandPalette {
    state: Entity<CommandState>,
    /// Attached by the shell once startup has opened the database and built
    /// the shared AniList client.
    db: Option<Db>,
    anilist: Option<AniList>,
    /// Local platform entries of both kinds, loaded when the palette opens.
    local: Vec<MediaSummary>,
    /// Remote rows for the current query; `None` while offline or waiting.
    remote: Option<Vec<MediaSummary>>,
    /// The debounce wait and the search it starts. Replacing it cancels the
    /// older request, so at most one is in flight (#33).
    search: Option<Task<()>>,
    local_load: Option<Task<()>>,
    /// Recent searches, most recent first, for the empty-query mode.
    history: Vec<String>,
}

impl EventEmitter<PaletteEvent> for CommandPalette {}

impl CommandPalette {
    pub(crate) fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        Self {
            state: cx.new(|cx| CommandState::new(window, cx)),
            db: None,
            anilist: None,
            local: Vec::new(),
            remote: None,
            search: None,
            local_load: None,
            history: Vec::new(),
        }
    }

    /// Gives the palette its account-free list source and catalog client.
    pub(crate) fn attach_sources(&mut self, db: Db, anilist: AniList) {
        self.db = Some(db);
        self.anilist = Some(anilist);
    }

    /// Starts over in `mode` and focuses the query field.
    pub(crate) fn open(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.state.update(cx, |state, cx| {
            state.set_query(mode.prefix(), window, cx);
            state.focus(window, cx);
        });
        self.search = None;
        self.remote = None;
        self.start_local_load(cx);
        cx.notify();
    }

    /// Loads the Local platform's list entries through the bridge. AniList
    /// ids from db let them be de-duplicated against remote results.
    fn start_local_load(&mut self, cx: &mut Context<'_, Self>) {
        let Some(db) = self.db.clone() else {
            return;
        };
        self.local_load = Some(cx.spawn(async move |palette, cx| {
            let load = cx.update(|cx| {
                bridge::run(cx, async move {
                    let mut summaries = Vec::new();
                    for kind in [MediaKind::Anime, MediaKind::Manga] {
                        if let Ok(entries) = db.local_list().entries(kind).await {
                            for entry in entries {
                                if let Ok(ids) = db.media().external_ids(entry.media).await {
                                    summaries.push(local_summary(kind, &ids));
                                }
                            }
                        }
                    }
                    summaries
                })
            });
            let rows = load.await.unwrap_or_default();
            let _ = palette.update(cx, |palette, cx| {
                palette.local = rows;
                cx.notify();
            });
        }));
    }

    /// A query changed: drop the older search, keep the recent-query list,
    /// then wait out the debounce before asking AniList.
    fn query_changed(&mut self, cx: &mut Context<'_, Self>) {
        let query = self.state.read(cx).query(cx).to_string();
        let (mode, text) = Mode::parse(&query);
        let text = text.to_owned();
        self.search = None;
        self.remote = None;
        if mode == Mode::Navigate || !needs_remote(&text) {
            cx.notify();
            return;
        }
        self.history = remember(&self.history, &query);
        let Some(anilist) = self.anilist.clone() else {
            cx.notify();
            return;
        };
        self.search = Some(cx.spawn(async move |palette, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let filters = SearchFilters {
                search: Some(text),
                kind: mode.kind(),
                ..SearchFilters::default()
            };
            let per_page = u32::try_from(MAX_RESULTS).unwrap_or(u32::MAX);
            let fetch = cx.update(|cx| {
                bridge::run(cx, async move {
                    anilist
                        .search(&filters, PageRequest::new(1, per_page))
                        .await
                })
            });
            // An error (including an unavailable AniList) leaves the local
            // matches as the only rows.
            let rows = fetch.await.ok().and_then(Result::ok).map(|page| page.media);
            let _ = palette.update(cx, |palette, cx| {
                palette.remote = rows;
                palette.search = None;
                cx.notify();
            });
        }));
        cx.notify();
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

    fn confirm(&mut self, entry: Entry, window: &mut Window, cx: &mut Context<'_, Self>) {
        match entry {
            Entry::Route(route) => cx.emit(PaletteEvent::Navigate(route)),
            Entry::Mode(mode) => self.open(mode, window, cx),
            // set_query fires the CommandState's on_query callback, which
            // already runs query_changed; calling it twice would start two
            // searches and write the recent-query list twice.
            Entry::Search(text) => {
                self.state
                    .update(cx, |state, cx| state.set_query(text, window, cx));
            }
            // A media route arrives with the Entry screen; the row is still
            // useful as a result today.
            Entry::Media(_) => {}
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

impl Render for CommandPalette {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let query = self.state.read(cx).query(cx);
        let (mode, rows) = entries_for(&query, &self.local, self.remote.as_deref(), &self.history);
        let on_query = cx.entity().downgrade();
        let on_confirm = cx.entity().downgrade();
        // The rows on screen, so a pick never lands in a newer query's list.
        let picked = rows.clone();
        div()
            .key_context(COMMAND_PALETTE)
            .capture_action(cx.listener(Self::cancel))
            .child(
                Command::new(&self.state)
                    .bordered(false)
                    .items(rows.into_iter().map(Entry::item))
                    // Search modes get their rows already matched (#63).
                    .filterable(mode == Mode::Navigate)
                    .placeholder("Go to a page, or type / for search modes")
                    .empty(move |_, _, cx| empty_state(mode, cx))
                    .on_query(move |_, _, cx| {
                        if let Some(palette) = on_query.upgrade() {
                            palette.update(cx, CommandPalette::query_changed);
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

fn empty_state(mode: Mode, cx: &App) -> impl IntoElement + use<> {
    let message = match mode {
        Mode::Navigate => "No page or mode matches",
        Mode::Search => "No local or AniList matches yet",
        Mode::Anime => "No anime matches yet",
        Mode::Manga => "No manga matches yet",
    };
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
    #![expect(
        clippy::indexing_slicing,
        reason = "test harness indexes recorded bodies and known rows"
    )]

    use std::sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Json, Router, extract::State, routing::post};
    use gpui_kit::{TestAppContext, WindowOptions};
    use serde_json::{Value, json};
    use tokio::{net::TcpListener, runtime::Builder};
    use ui_state::bridge;

    use super::*;

    #[derive(Clone)]
    struct MockState {
        count: Arc<AtomicUsize>,
        searches: Arc<Mutex<Vec<String>>>,
    }

    #[gpui_kit::test]
    async fn a_newer_query_cancels_the_older_request(cx: &mut TestAppContext) {
        let state = MockState {
            count: Arc::new(AtomicUsize::new(0)),
            searches: Arc::new(Mutex::new(Vec::new())),
        };
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(state): State<MockState>, Json(body): Json<Value>| async move {
                        state.count.fetch_add(1, Ordering::SeqCst);
                        state.searches.lock().unwrap().push(
                            body["variables"]["search"]
                                .as_str()
                                .unwrap_or_default()
                                .to_owned(),
                        );
                        Json(json!({"data": {"Page": {"pageInfo": {"hasNextPage": false}, "media": []}}}))
                    },
                ),
            )
            .with_state(state.clone());
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .worker_threads(1)
            .build()
            .unwrap();
        let _guard = runtime.enter();
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let (window, palette) = cx.update(|cx| {
            gpui_kit::init(cx);
            ui_theme::init(cx);
            gpui_kit::open_window(WindowOptions::default(), cx, |window, cx| {
                cx.new(|cx| CommandPalette::new(window, cx))
            })
            .unwrap()
        });
        let http = net::Client::new().unwrap();
        palette.update(cx, |palette, _| {
            palette.anilist = Some(AniList::with_endpoint(http, format!("http://{address}/")));
        });

        // Two keystroke pauses in quick succession: the first wait must be
        // dropped, so only the last query reaches AniList.
        for text in ["/anime ab", "/anime abc"] {
            cx.update_window(window, |_, window, cx| {
                palette.update(cx, |palette, cx| {
                    palette
                        .state
                        .update(cx, |state, cx| state.set_query(text, window, cx));
                    palette.query_changed(cx);
                });
            })
            .unwrap();
        }
        // The timer runs on the background executor, so let real time pass
        // before draining the foreground tasks.
        tokio::time::sleep(DEBOUNCE / 2).await;
        cx.run_until_parked();
        assert_eq!(
            state.count.load(Ordering::SeqCst),
            0,
            "no request before the debounce elapses"
        );
        tokio::time::sleep(DEBOUNCE * 2).await;
        cx.run_until_parked();

        assert_eq!(
            state.count.load(Ordering::SeqCst),
            1,
            "one request per pause"
        );
        assert_eq!(*state.searches.lock().unwrap(), ["abc"]);
    }
}
