//! Advanced Search screen: the full decision #23 filter set in the route, a
//! virtualized result grid and one committed request per filter or page
//! (ticket #59).

use std::time::Duration;

use domain::MediaKind;
use gpui_kit::{
    App, AppContext, Context, Entity, EventEmitter, InteractiveElement, IntoElement, ParentElement,
    Render, Resource, Styled, Subscription, Task, TestSupportExt, Window,
    component::{
        ActiveTheme, Disableable,
        button::Button,
        input::{Input, InputEvent, InputState},
        switch::Switch,
    },
    div,
};
use metadata::AniList;
use ui_components::{MediaCardView, MediaFormat, PosterGrid};
use ui_state::{
    SearchFilters, SearchFormat, SearchSeason, SearchSession, SearchSort, SearchStatus, bridge,
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
    grid: Entity<PosterGrid>,
    query: Entity<InputState>,
    genres: Entity<InputState>,
    country: Entity<InputState>,
    _subscriptions: Vec<Subscription>,
    error: Option<String>,
    task: Option<Task<()>>,
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
        Some(SearchFormat::Tv) => Some(SearchFormat::Movie),
        Some(SearchFormat::Movie) => Some(SearchFormat::Ova),
        Some(SearchFormat::Ova) => Some(SearchFormat::Manga),
        _ => None,
    }
}

fn next_status(current: Option<SearchStatus>) -> Option<SearchStatus> {
    match current {
        None => Some(SearchStatus::Releasing),
        Some(SearchStatus::Releasing) => Some(SearchStatus::Finished),
        Some(SearchStatus::Finished) => Some(SearchStatus::NotYetReleased),
        Some(SearchStatus::NotYetReleased) => Some(SearchStatus::Hiatus),
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
            let q = query.clone();
            let g = genres.clone();
            let c = country.clone();
            let mut subscriptions = Vec::new();
            for input in [&query, &genres, &country] {
                let q = q.clone();
                let g = g.clone();
                let c = c.clone();
                subscriptions.push(cx.subscribe(
                    input,
                    move |screen: &mut Self, _, event: &InputEvent, cx| {
                        if matches!(event, InputEvent::Change) {
                            screen.schedule_apply(&q, &g, &c, cx);
                        }
                    },
                ));
            }
            Self {
                session: SearchSession::new(AniListCatalog::new(anilist), filters),
                grid: PosterGrid::new(Vec::new(), cx),
                query,
                genres,
                country,
                _subscriptions: subscriptions,
                error: None,
                task: None,
            }
        });
        screen.update(cx, SearchScreen::apply_now);
        screen
    }

    /// Applies filters from a restored route without navigating again.
    pub(crate) fn set_filters(&mut self, filters: SearchFilters, cx: &mut Context<'_, Self>) {
        if self.session.filters() != &filters {
            // The route is authoritative on Back; re-fetch only when it
            // differs from what the session already committed.
            let mut session = self.session.clone();
            self.task = Some(cx.spawn(async move |screen, cx| {
                let task = cx.update(|cx| {
                    bridge::run(
                        cx,
                        async move { session.apply(filters).await.map(|()| session) },
                    )
                });
                let result = task.await;
                let _ = screen.update(cx, |screen, cx| {
                    if let Ok(Ok(session)) = result {
                        screen.session = session;
                        screen.sync_grid(cx);
                        cx.emit(SearchEvent::Loaded);
                    }
                    cx.notify();
                });
            }));
        }
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
        filters
    }

    /// Debounces typed filters; the retained Task cancels an older commit.
    fn schedule_apply(
        &mut self,
        query: &Entity<InputState>,
        genres: &Entity<InputState>,
        country: &Entity<InputState>,
        cx: &mut Context<'_, Self>,
    ) {
        let filters = self.collected(query, genres, country, cx);
        let mut session = self.session.clone();
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
                    screen.error = None;
                    screen.sync_grid(cx);
                    cx.emit(SearchEvent::Loaded);
                    cx.emit(SearchEvent::Filters(screen.session.filters().clone()));
                    cx.notify();
                }
                Ok(Err(error)) => {
                    screen.error = Some(error.to_string());
                    cx.notify();
                }
                Err(error) => {
                    screen.error = Some(error.to_string());
                    cx.notify();
                }
            });
        }));
    }

    /// Commits the current filters once, used for the initial page and for
    /// filter buttons.
    fn apply_now(&mut self, cx: &mut Context<'_, Self>) {
        let filters = self.session.filters().clone();
        let mut session = self.session.clone();
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
                        screen.error = None;
                        screen.sync_grid(cx);
                        cx.emit(SearchEvent::Loaded);
                    }
                    Ok(Err(error)) => screen.error = Some(error.to_string()),
                    Err(error) => screen.error = Some(error.to_string()),
                }
                cx.notify();
            });
        }));
    }

    fn change<F>(&mut self, cx: &mut Context<'_, Self>, update: F)
    where
        F: FnOnce(&mut SearchFilters),
    {
        let mut filters = self.session.filters().clone();
        update(&mut filters);
        let catalog = self.session.catalog().clone();
        self.session = SearchSession::new(catalog, filters.clone());
        self.apply_now(cx);
        cx.emit(SearchEvent::Filters(filters));
    }

    /// Loads one more page; the retained Task cancels an older page request.
    fn next_page(&mut self, cx: &mut Context<'_, Self>) {
        let mut session = self.session.clone();
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
                        screen.error = None;
                        screen.sync_grid(cx);
                        cx.emit(SearchEvent::Loaded);
                    }
                    Ok(Err(error)) => screen.error = Some(error.to_string()),
                    Err(error) => screen.error = Some(error.to_string()),
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
            .child(
                div().p_4().flex().justify_center().child(
                    Button::new("search-more")
                        .label(format!("Load more (page {})", self.session.page() + 1))
                        .disabled(!self.session.has_next())
                        .on_click(cx.listener(|screen: &mut Self, _, _, cx| {
                            screen.next_page(cx);
                        })),
                ),
            )
            .test_support()
    }
}
