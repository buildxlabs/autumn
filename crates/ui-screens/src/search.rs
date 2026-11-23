//! Advanced Search screen: the full decision #23 filter set in the route, a
//! virtualized result grid and one committed request per filter or page
//! (ticket #59).

use std::{collections::HashMap, time::Duration};

use domain::MediaKind;
use gpui_kit::{
    AnyWindowHandle, App, AppContext, Context, Entity, EventEmitter, InteractiveElement,
    IntoElement, ParentElement, Pixels, Point, Render, Resource, Styled, Subscription, Task,
    TestSupportExt, Window,
    component::{
        ActiveTheme,
        button::Button,
        input::{Input, InputEvent, InputState},
        switch::Switch,
    },
    div,
};
use metadata::AniList;
use ui_components::{MediaCardView, MediaFormat, PosterGrid};
use ui_state::{
    RestoredPage, SearchFilters, SearchFormat, SearchSeason, SearchSession, SearchSort,
    SearchStatus, bridge,
};

use crate::search_catalog::AniListCatalog;

/// How long a typed filter waits before it is committed as one request
/// (decision #23).
const DEBOUNCE: Duration = Duration::from_millis(350);

/// What the screen asks its host to do.
#[derive(Debug, Clone)]
pub(crate) enum SearchEvent {
    /// The committed filters changed; the shell updates the route.
    Filters(SearchFilters),
    /// Results arrived, so a saved scroll offset can be re-applied.
    Loaded,
}

/// The Advanced Search view.
pub(crate) struct SearchScreen {
    session: SearchSession<AniListCatalog>,
    /// Last loaded pages per committed filter set, so Back restores a prior
    /// search without re-fetching or clamping its scroll offset.
    cache: HashMap<SearchFilters, RestoredPage>,
    /// The window handle lets a restored route write text into the inputs
    /// even when the shell navigates without a `Window` in hand.
    window: AnyWindowHandle,
    grid: Entity<PosterGrid>,
    query: Entity<InputState>,
    genres: Entity<InputState>,
    country: Entity<InputState>,
    year: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    error: Option<String>,
    task: Option<Task<()>>,
    /// The page a request is in flight for, so scrolling cannot start two.
    requested_page: u32,
    /// The saved grid offset to re-apply once restored results are drawn.
    scroll_pending: Option<Point<Pixels>>,
}

impl std::fmt::Debug for SearchScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SearchScreen")
            .field("filters", self.session.filters())
            .field("results", &self.session.results().len())
            .field("page", &self.session.page())
            .finish_non_exhaustive()
    }
}

impl EventEmitter<SearchEvent> for SearchScreen {}

fn next_kind(current: Option<MediaKind>) -> Option<MediaKind> {
    match current {
        None => Some(MediaKind::Anime),
        Some(MediaKind::Anime) => Some(MediaKind::Manga),
        _ => None,
    }
}

fn next_sort(current: Option<SearchSort>) -> Option<SearchSort> {
    match current {
        None => Some(SearchSort::Match),
        Some(SearchSort::Match) => Some(SearchSort::Trending),
        Some(SearchSort::Trending) => Some(SearchSort::Popularity),
        Some(SearchSort::Popularity) => Some(SearchSort::Score),
        Some(SearchSort::Score) => Some(SearchSort::Favourites),
        Some(SearchSort::Favourites) => Some(SearchSort::StartDate),
        Some(SearchSort::StartDate) => Some(SearchSort::UpdatedAt),
        _ => None,
    }
}

fn next_format(current: Option<SearchFormat>) -> Option<SearchFormat> {
    match current {
        None => Some(SearchFormat::Tv),
        Some(SearchFormat::Tv) => Some(SearchFormat::TvShort),
        Some(SearchFormat::TvShort) => Some(SearchFormat::Movie),
        Some(SearchFormat::Movie) => Some(SearchFormat::Special),
        Some(SearchFormat::Special) => Some(SearchFormat::Ova),
        Some(SearchFormat::Ova) => Some(SearchFormat::Ona),
        Some(SearchFormat::Ona) => Some(SearchFormat::Music),
        Some(SearchFormat::Music) => Some(SearchFormat::Manga),
        Some(SearchFormat::Manga) => Some(SearchFormat::Novel),
        Some(SearchFormat::Novel) => Some(SearchFormat::OneShot),
        _ => None,
    }
}

fn next_status(current: Option<SearchStatus>) -> Option<SearchStatus> {
    match current {
        None => Some(SearchStatus::Releasing),
        Some(SearchStatus::Releasing) => Some(SearchStatus::Finished),
        Some(SearchStatus::Finished) => Some(SearchStatus::NotYetReleased),
        Some(SearchStatus::NotYetReleased) => Some(SearchStatus::Cancelled),
        Some(SearchStatus::Cancelled) => Some(SearchStatus::Hiatus),
        _ => None,
    }
}

fn next_season(current: Option<SearchSeason>) -> Option<SearchSeason> {
    match current {
        None => Some(SearchSeason::Winter),
        Some(SearchSeason::Winter) => Some(SearchSeason::Spring),
        Some(SearchSeason::Spring) => Some(SearchSeason::Summer),
        Some(SearchSeason::Summer) => Some(SearchSeason::Fall),
        _ => None,
    }
}

fn label<T: std::fmt::Debug>(name: &str, value: Option<&T>) -> String {
    value.map_or_else(
        || format!("{name}: any"),
        |value| format!("{name}: {value:?}"),
    )
}

fn card_view(media: &ui_state::MediaPreview) -> MediaCardView {
    MediaCardView {
        poster: media
            .poster
            .as_ref()
            .map(|url| Resource::Uri(url.clone().into())),
        title: media.title.clone().into(),
        format: match media.format {
            Some(SearchFormat::Manga | SearchFormat::Novel | SearchFormat::OneShot) => {
                Some(MediaFormat::Manga)
            }
            Some(_) => Some(MediaFormat::Anime),
            None => None,
        },
        score: media.score.map(f32::from),
        progress: 0,
        total: None,
        detail: media.title.clone().into(),
    }
}

/// Plain text restored into the four filter inputs.
struct InputText {
    query: String,
    genres: String,
    country: String,
    year: String,
}

/// The input handles, cloned so restoration can be deferred to another turn.
struct InputEntities {
    query: Entity<InputState>,
    genres: Entity<InputState>,
    country: Entity<InputState>,
    year: Entity<InputState>,
}

impl InputEntities {
    fn write(&self, text: &InputText, window: &mut Window, cx: &mut App) {
        self.query.update(cx, |input, cx| {
            input.set_value(text.query.clone(), window, cx);
        });
        self.genres.update(cx, |input, cx| {
            input.set_value(text.genres.clone(), window, cx);
        });
        self.country.update(cx, |input, cx| {
            input.set_value(text.country.clone(), window, cx);
        });
        self.year.update(cx, |input, cx| {
            input.set_value(text.year.clone(), window, cx);
        });
    }
}

impl SearchScreen {
    /// Creates the screen with the filters carried by the route.
    pub(crate) fn new(
        filters: SearchFilters,
        anilist: AniList,
        window: &mut Window,
        cx: &mut App,
    ) -> Entity<Self> {
        let screen = cx.new(|cx| {
            let query = cx.new(|cx| InputState::new(window, cx).placeholder("Search titles"));
            let genres =
                cx.new(|cx| InputState::new(window, cx).placeholder("Genres, comma separated"));
            let country = cx.new(|cx| InputState::new(window, cx).placeholder("Country, e.g. JP"));
            let year = cx.new(|cx| InputState::new(window, cx).placeholder("Year"));
            let mut subscriptions = Vec::new();
            for input in [&query, &genres, &country, &year] {
                let q = query.clone();
                let g = genres.clone();
                let c = country.clone();
                let y = year.clone();
                subscriptions.push(cx.subscribe(
                    input,
                    move |screen: &mut Self, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            screen.schedule_apply(&q, &g, &c, &y, cx);
                        }
                    },
                ));
            }
            Self {
                session: SearchSession::new(AniListCatalog::new(anilist), filters),
                cache: HashMap::new(),
                window: window.window_handle(),
                grid: PosterGrid::new(Vec::new(), cx),
                query,
                genres,
                country,
                year,
                _subscriptions: subscriptions,
                error: None,
                task: None,
                requested_page: 0,
                scroll_pending: None,
            }
        });
        let weak = screen.downgrade();
        screen.update(cx, |screen, cx| {
            screen.grid.update(cx, |grid, cx| {
                grid.on_near_end(move |_, app| {
                    let _ = weak.update(app, SearchScreen::maybe_next_page);
                });
                cx.notify();
            });
        });
        // No request here: creation happens at startup. The first page is
        // fetched when the Search route is first shown.
        screen
    }

    /// Applies filters from a restored route without navigating again.
    pub(crate) fn set_filters(&mut self, filters: SearchFilters, cx: &mut Context<'_, Self>) {
        self.write_inputs(&filters, cx);
        let first_show = self.session.page() == 0;
        if self.session.filters() == &filters && !first_show {
            return;
        }
        if let Some(loaded) = self.cache.get(&filters).cloned() {
            self.session.restore(filters, loaded);
            self.requested_page = self.session.page();
            self.error = None;
            self.sync_grid(cx);
            cx.emit(SearchEvent::Loaded);
            cx.notify();
            return;
        }
        self.load(filters, cx);
    }

    /// Writes restored text into the inputs. `set_value` emits no Change
    /// event, so the next keystroke sees the restored text without re-applying
    /// the filters here.
    fn write_inputs(&self, filters: &SearchFilters, cx: &mut Context<'_, Self>) {
        let text = InputText {
            query: filters.query.clone().unwrap_or_default(),
            genres: filters.genres.join(", "),
            country: filters.country.clone().unwrap_or_default(),
            year: filters
                .year
                .map_or_else(String::new, |year| year.to_string()),
        };
        let inputs = self.input_entities();
        let window = self.window;
        if window
            .update(cx, |_, window, app| inputs.write(&text, window, app))
            .is_err()
        {
            // A Back keypress is handled inside `update_window`, which locks
            // the window against a nested update. Retry once it is free.
            cx.spawn(async move |_, cx| {
                let _ = window.update(cx, |_, window, app| inputs.write(&text, window, app));
            })
            .detach();
        }
    }

    fn input_entities(&self) -> InputEntities {
        InputEntities {
            query: self.query.clone(),
            genres: self.genres.clone(),
            country: self.country.clone(),
            year: self.year.clone(),
        }
    }

    /// Fetches page one for `filters` without publishing a route change.
    fn load(&mut self, filters: SearchFilters, cx: &mut Context<'_, Self>) {
        let mut session = self.session.clone();
        self.requested_page = 1;
        self.task = Some(cx.spawn(async move |screen, cx| {
            let task = cx.update(|cx| {
                bridge::run(
                    cx,
                    async move { session.apply(filters).await.map(|()| session) },
                )
            });
            let result = task.await;
            let _ = screen.update(cx, |screen, cx| {
                match result {
                    Ok(Ok(session)) => {
                        screen.session = session;
                        screen.requested_page = screen.session.page();
                        screen.error = None;
                        screen.remember();
                        screen.sync_grid(cx);
                        cx.emit(SearchEvent::Loaded);
                    }
                    Ok(Err(error)) => {
                        screen.requested_page = screen.session.page();
                        screen.error = Some(error.to_string());
                    }
                    Err(error) => {
                        screen.requested_page = screen.session.page();
                        screen.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Test seam: the query currently shown in the input.
    #[cfg(test)]
    pub(crate) fn query_text(&self, cx: &App) -> String {
        self.query.read(cx).value().to_string()
    }

    /// Test seam: types into the query without going through the keyboard.
    #[cfg(test)]
    pub(crate) fn set_query_text(&self, text: &str, cx: &mut Context<'_, Self>) {
        let query = self.query.clone();
        let value = text.to_owned();
        let window = self.window;
        let _ = window.update(cx, |_, window, app| {
            query.update(app, |input, cx| input.set_value(value, window, cx));
        });
    }

    /// The grid's scroll offset, saved into the route when leaving Search.
    pub(crate) fn scroll_offset(&self, cx: &App) -> Point<Pixels> {
        self.grid.read(cx).scroll_offset()
    }

    /// Sets a saved offset and keeps it for the next `content_loaded`.
    pub(crate) fn restore_scroll(&mut self, offset: Point<Pixels>, cx: &mut Context<'_, Self>) {
        self.grid.read(cx).set_scroll_offset(offset);
        self.scroll_pending = Some(offset);
    }

    /// Re-applies the saved offset once restored results have been drawn.
    pub(crate) fn content_loaded(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(offset) = self.scroll_pending.take() {
            self.grid.read(cx).set_scroll_offset(offset);
        }
    }

    /// Pages when the user scrolls near the end of the loaded results.
    fn maybe_next_page(&mut self, cx: &mut Context<'_, Self>) {
        if self.session.has_next() && self.session.page() >= self.requested_page {
            self.next_page(cx);
        }
    }

    fn remember(&mut self) {
        self.cache.insert(
            self.session.filters().clone(),
            RestoredPage {
                results: self.session.results().to_vec(),
                page: self.session.page(),
                has_next: self.session.has_next(),
            },
        );
    }

    fn sync_grid(&mut self, cx: &mut Context<'_, Self>) {
        let views = self.session.results().iter().map(card_view).collect();
        self.grid.update(cx, |grid, cx| grid.set_items(views, cx));
    }

    fn collected(
        &self,
        query: &Entity<InputState>,
        genres: &Entity<InputState>,
        country: &Entity<InputState>,
        year: &Entity<InputState>,
        cx: &App,
    ) -> SearchFilters {
        let mut filters = self.session.filters().clone();
        let text = query.read(cx).value().to_string();
        filters.query = (!text.is_empty()).then_some(text);
        let genre_text = genres.read(cx).value().to_string();
        filters.genres = genre_text
            .split(',')
            .map(str::trim)
            .filter(|genre| !genre.is_empty())
            .map(str::to_owned)
            .collect();
        let country_text = country.read(cx).value().to_string();
        filters.country = (!country_text.is_empty()).then_some(country_text);
        let year_text = year.read(cx).value().to_string();
        filters.year = year_text.parse::<u16>().ok();
        filters
    }

    /// Debounces typed filters; the retained Task cancels an older commit.
    fn schedule_apply(
        &mut self,
        query: &Entity<InputState>,
        genres: &Entity<InputState>,
        country: &Entity<InputState>,
        year: &Entity<InputState>,
        cx: &mut Context<'_, Self>,
    ) {
        let filters = self.collected(query, genres, country, year, cx);
        let mut session = self.session.clone();
        self.requested_page = 1;
        self.task = Some(cx.spawn(async move |screen, cx| {
            cx.background_executor().timer(DEBOUNCE).await;
            let task = cx.update(|cx| {
                bridge::run(
                    cx,
                    async move { session.apply(filters).await.map(|()| session) },
                )
            });
            let result = task.await;
            let _ = screen.update(cx, |screen, cx| match result {
                Ok(Ok(session)) => {
                    screen.session = session;
                    screen.requested_page = screen.session.page();
                    screen.error = None;
                    screen.remember();
                    cx.emit(SearchEvent::Filters(screen.session.filters().clone()));
                    screen.sync_grid(cx);
                    cx.emit(SearchEvent::Loaded);
                    cx.notify();
                }
                Ok(Err(error)) => {
                    screen.requested_page = screen.session.page();
                    screen.error = Some(error.to_string());
                    cx.notify();
                }
                Err(error) => {
                    screen.requested_page = screen.session.page();
                    screen.error = Some(error.to_string());
                    cx.notify();
                }
            });
        }));
    }

    /// Commits `filters` without discarding the last good results until the
    /// catalog answers. The returned Task replaces the old one, cancelling an
    /// earlier filter or page request.
    fn apply_with(&mut self, filters: SearchFilters, cx: &mut Context<'_, Self>) {
        let mut session = self.session.clone();
        self.requested_page = 1;
        self.task = Some(cx.spawn(async move |screen, cx| {
            let task = cx.update(|cx| {
                bridge::run(
                    cx,
                    async move { session.apply(filters).await.map(|()| session) },
                )
            });
            let result = task.await;
            let _ = screen.update(cx, |screen, cx| {
                match result {
                    Ok(Ok(session)) => {
                        screen.session = session;
                        screen.requested_page = screen.session.page();
                        screen.error = None;
                        screen.remember();
                        // Publish the route before resizing the grid: the
                        // shell saves the scroll offset on navigation.
                        cx.emit(SearchEvent::Filters(screen.session.filters().clone()));
                        screen.sync_grid(cx);
                        cx.emit(SearchEvent::Loaded);
                    }
                    Ok(Err(error)) => {
                        screen.requested_page = screen.session.page();
                        screen.error = Some(error.to_string());
                    }
                    Err(error) => {
                        screen.requested_page = screen.session.page();
                        screen.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
    }

    fn change<F>(&mut self, cx: &mut Context<'_, Self>, update: F)
    where
        F: FnOnce(&mut SearchFilters),
    {
        // Fold in text still waiting out the debounce, so a quick button
        // click cannot drop the last typed edit.
        let (q, g, c, y) = (
            self.query.clone(),
            self.genres.clone(),
            self.country.clone(),
            self.year.clone(),
        );
        let mut filters = self.collected(&q, &g, &c, &y, cx);
        update(&mut filters);
        self.apply_with(filters, cx);
    }

    /// Loads one more page; the retained Task cancels an older page request.
    fn next_page(&mut self, cx: &mut Context<'_, Self>) {
        let mut session = self.session.clone();
        self.requested_page = session.page() + 1;
        self.task = Some(cx.spawn(async move |screen, cx| {
            let task = cx.update(|cx| {
                bridge::run(
                    cx,
                    async move { session.next_page().await.map(|()| session) },
                )
            });
            let result = task.await;
            let _ = screen.update(cx, |screen, cx| {
                match result {
                    Ok(Ok(session)) => {
                        screen.session = session;
                        screen.requested_page = screen.session.page();
                        screen.error = None;
                        screen.remember();
                        screen.sync_grid(cx);
                        cx.emit(SearchEvent::Loaded);
                    }
                    Ok(Err(error)) => {
                        screen.requested_page = screen.session.page();
                        screen.error = Some(error.to_string());
                    }
                    Err(error) => {
                        screen.requested_page = screen.session.page();
                        screen.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        }));
    }
}

impl Render for SearchScreen {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let filters = self.session.filters().clone();
        div()
            .id("search-screen")
            .size_full()
            .flex()
            .flex_col()
            .child(div().px_8().pt_8().text_3xl().child("Advanced Search"))
            .child(
                div()
                    .px_8()
                    .pt_4()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(Input::new(&self.query).aria_label("Search query"))
                    .child(
                        Button::new("search-kind")
                            .label(label("Type", filters.kind.as_ref()))
                            .on_click(cx.listener(|screen: &mut Self, _, _, cx| {
                                screen.change(cx, |f| f.kind = next_kind(f.kind));
                            })),
                    )
                    .child(
                        Button::new("search-sort")
                            .label(label("Sort", filters.sort.as_ref()))
                            .on_click(cx.listener(|screen: &mut Self, _, _, cx| {
                                screen.change(cx, |f| f.sort = next_sort(f.sort));
                            })),
                    )
                    .child(
                        Button::new("search-format")
                            .label(label("Format", filters.format.as_ref()))
                            .on_click(cx.listener(|screen: &mut Self, _, _, cx| {
                                screen.change(cx, |f| f.format = next_format(f.format));
                            })),
                    ),
            )
            .child(
                div()
                    .px_8()
                    .pt_2()
                    .flex()
                    .gap_3()
                    .items_center()
                    .child(Input::new(&self.genres).aria_label("Genres"))
                    .child(Input::new(&self.country).aria_label("Country"))
                    .child(Input::new(&self.year).aria_label("Season year"))
                    .child(
                        Button::new("search-status")
                            .label(label("Status", filters.status.as_ref()))
                            .on_click(cx.listener(|screen: &mut Self, _, _, cx| {
                                screen.change(cx, |f| f.status = next_status(f.status));
                            })),
                    )
                    .child(
                        Button::new("search-season")
                            .label(label("Season", filters.season.as_ref()))
                            .on_click(cx.listener(|screen: &mut Self, _, _, cx| {
                                screen.change(cx, |f| f.season = next_season(f.season));
                            })),
                    )
                    .child(
                        Switch::new("search-adult")
                            .label("Adult")
                            .checked(filters.adult)
                            .on_click(cx.listener(|screen: &mut Self, checked, _, cx| {
                                screen.change(cx, |f| f.adult = *checked);
                            })),
                    ),
            )
            .children(self.error.as_ref().map(|error| {
                div()
                    .px_8()
                    .pt_2()
                    .text_color(colors.danger)
                    .child(error.clone())
            }))
            .child(div().flex_1().min_h_0().child(self.grid.clone()))
            .test_support()
    }
}
