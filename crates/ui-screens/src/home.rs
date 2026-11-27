//! The Home route (#61): the fixed widget set from #23, in order, each
//! section with its own loader, error card and offline banner.

use std::rc::Rc;

use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, InteractiveElement, IntoElement, ParentElement,
    Resource, SharedString, Styled, Subscription, TestSupportExt, Window,
    component::{ActiveTheme, button::Button, carousel::CarouselState},
    div, px,
};
use metadata::MediaSummary;
use ui_components::{
    CarouselWheel, EpisodeActions, EpisodeCard, EpisodeCardView, LruImageCache, MEDIA_CARD_HEIGHT,
    MediaCard, MediaCardView, MediaCarousel, PosterGrid,
    fetch_status::{FetchError, error_card, fetch_loader, offline_banner},
    hero::{HeroBannerData, hero_banner},
};
use ui_state::{
    Route,
    actions::{OpenLibraryExplorer, ScanLibrary},
    fetch_status::cache_age,
    home_store::{
        ContinueEpisode, HomeEntry, HomeSection, HomeStore, LibraryGroup, LibraryStats,
        SectionData, SectionState,
    },
    library_store::{LibraryStore, ScanState},
    playback_store::PlaybackStore,
};

use crate::{
    discover::{media_card, title_of},
    shell::AppShell,
};

/// The Home sections that render as carousels, in widget order.
const CAROUSELS: [HomeSection; 4] = [
    HomeSection::MyLists,
    HomeSection::MissedSequels,
    HomeSection::AiredRecently,
    HomeSection::AnimeCarousel,
];

/// Paging state for Home's carousels, owned here so it survives redraws.
pub(crate) struct HomeCarousels {
    states: Vec<Entity<CarouselState>>,
    wheels: Vec<CarouselWheel>,
    /// The Local Library's virtualized poster grid.
    local_grid: Entity<PosterGrid>,
}

impl HomeCarousels {
    /// One paging state per carousel section, plus the poster grid.
    pub(crate) fn new(cx: &mut App) -> Entity<Self> {
        let states = CAROUSELS
            .iter()
            .map(|_| cx.new(|_| CarouselState::new(0)))
            .collect();
        let wheels = CAROUSELS.iter().map(|_| CarouselWheel::default()).collect();
        let local_grid = PosterGrid::new(Vec::new(), cx);
        cx.new(|_| Self {
            states,
            wheels,
            local_grid,
        })
    }

    fn slot(&self, section: HomeSection) -> Option<(Entity<CarouselState>, CarouselWheel)> {
        let index = CAROUSELS.iter().position(|item| *item == section)?;
        Some((
            self.states.get(index)?.clone(),
            self.wheels.get(index)?.clone(),
        ))
    }
}

/// Tells each carousel how many cards its section holds whenever the store
/// changes, so paging works. `render` stays pure: the entity writes are here.
pub(crate) fn observe(
    store: &Entity<HomeStore>,
    carousels: &Entity<HomeCarousels>,
    cx: &mut Context<'_, AppShell>,
) -> Subscription {
    let carousels = carousels.clone();
    let grid = carousels.read(cx).local_grid.clone();
    let shell = cx.entity().downgrade();
    cx.observe(store, move |_, store, cx| {
        let counts: Vec<usize> = CAROUSELS
            .iter()
            .map(|section| section_cards(store.read(cx).state(*section)).0.len())
            .collect();
        let states = carousels.read(cx).states.clone();
        for (state, count) in states.iter().zip(counts) {
            state.update(cx, |state, cx| state.set_item_count(count, cx));
        }
        let (cards, ids) = match store.read(cx).state(HomeSection::LocalAnimeLibrary) {
            SectionState::Ready(SectionData::LocalAnimeLibrary(media))
            | SectionState::Offline(SectionData::LocalAnimeLibrary(media), _) => (
                media
                    .iter()
                    .map(|item| MediaCardView {
                        title: item
                            .title
                            .clone()
                            .unwrap_or_else(|| format!("Media {}", item.media.get()))
                            .into(),
                        ..MediaCardView::default()
                    })
                    .collect(),
                media.iter().map(|item| item.anilist_id).collect(),
            ),
            _ => (Vec::new(), Vec::new()),
        };
        let shell = shell.clone();
        grid.update(cx, |grid, cx| {
            grid.set_items(cards, cx);
            grid.on_open(move |index, _, cx| {
                if let Some(Some(id)) = ids.get(index).copied() {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.navigate(Route::Entry { id, tab: None }, cx);
                    });
                }
            });
        });
    })
}

/// Renders the Home route from the store's current state.
pub(crate) fn render(
    store: &Entity<HomeStore>,
    carousels: &Entity<HomeCarousels>,
    cache: &Entity<LruImageCache>,
    library_store: &Entity<LibraryStore>,
    playback_store: &Entity<PlaybackStore>,
    cx: &mut Context<'_, AppShell>,
) -> impl IntoElement {
    let states = store.read(cx).states().to_vec();
    let mut sections = Vec::with_capacity(states.len());
    for (section, state) in HomeSection::ALL.into_iter().zip(states) {
        sections.push(render_section(
            section,
            state,
            store,
            carousels,
            cache,
            playback_store,
            cx,
        ));
    }
    let scan_label = match library_store.read(cx).scan() {
        ScanState::Running {
            files_walked,
            files_matched,
        } => format!("Scanning… {files_walked} walked, {files_matched} matched"),
        _ => "Scan library".to_owned(),
    };
    div()
        .id("home-view")
        .test_support()
        .flex()
        .flex_col()
        .gap_6()
        .p_6()
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(Button::new("home-scan-library").label(scan_label).on_click(
                    |_, window, cx| {
                        window.dispatch_action(Box::new(ScanLibrary), cx);
                    },
                ))
                .child(
                    Button::new("home-library-explorer")
                        .label("Library Explorer")
                        .on_click(|_, window, cx| {
                            window.dispatch_action(Box::new(OpenLibraryExplorer), cx);
                        }),
                ),
        )
        .children(sections)
}

/// One section: its loader, error card, or data under an optional offline
/// banner.
fn render_section(
    section: HomeSection,
    state: SectionState,
    store: &Entity<HomeStore>,
    carousels: &Entity<HomeCarousels>,
    cache: &Entity<LruImageCache>,
    playback_store: &Entity<PlaybackStore>,
    cx: &mut Context<'_, AppShell>,
) -> gpui_kit::AnyElement {
    let id = section_id(section);
    if matches!(state, SectionState::Hidden) {
        return div().id(id).test_support().into_any_element();
    }
    let body = match state {
        SectionState::Loading => div().child(fetch_loader()).into_any_element(),
        SectionState::Hidden => div().into_any_element(),
        SectionState::Failed(message) => {
            let store = store.clone();
            error_card(
                SharedString::from(format!("{id}-error")),
                FetchError::Request(&message),
                move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
                    store.update(cx, |store, cx| store.retry(section, cx));
                },
            )
            .into_any_element()
        }
        SectionState::Ready(data) => {
            render_data(section, &data, carousels, cache, playback_store, cx)
        }
        SectionState::Offline(data, updated_at) => div()
            .flex()
            .flex_col()
            .gap_2()
            .child(offline_banner(&cache_age(updated_at)))
            .child(render_data(
                section,
                &data,
                carousels,
                cache,
                playback_store,
                cx,
            ))
            .into_any_element(),
    };
    let mut container = div().id(id).test_support().flex().flex_col().gap_2();
    if let Some(title) = section_title(section) {
        container = container.child(div().text_xl().child(title));
    }
    container.child(body).into_any_element()
}

#[expect(
    clippy::too_many_lines,
    reason = "the renderer owns one match arm per Home widget and its empty state"
)]
fn render_data(
    section: HomeSection,
    data: &SectionData,
    carousels: &Entity<HomeCarousels>,
    cache: &Entity<LruImageCache>,
    playback_store: &Entity<PlaybackStore>,
    cx: &mut Context<'_, AppShell>,
) -> gpui_kit::AnyElement {
    let colors = cx.theme().colors;
    match data {
        SectionData::ContinueWatching(entries) => match entries.first() {
            Some(first) => hero(
                "home-continue",
                hero_data(first),
                first.media.as_ref().map(|media| media.anilist_id),
                "Continue",
                cache,
                cx,
            ),
            None => empty("Nothing in progress. Add a show to your list from its Entry page."),
        },
        SectionData::AnimeContinueWatching(episodes) => {
            if episodes.is_empty() {
                return empty("Nothing to continue. Scan your library to find episodes.");
            }
            div()
                .flex()
                .gap_3()
                .children(
                    episodes
                        .iter()
                        .enumerate()
                        .map(|(index, episode)| continue_card(episode, index, playback_store)),
                )
                .into_any_element()
        }
        SectionData::DiscoverHeader(media) => match media.first() {
            Some(first) => hero(
                "home-discover",
                HeroBannerData {
                    title: title_of(first).into(),
                    synopsis: SharedString::default(),
                    banner: first.banner.clone(),
                },
                Some(first.anilist_id),
                "Details",
                cache,
                cx,
            ),
            None => empty("Nothing trending right now."),
        },
        SectionData::AnimeLibrary(groups) => {
            if groups.is_empty() {
                return empty("No local files on your lists yet.");
            }
            div()
                .flex()
                .flex_col()
                .gap_4()
                .children(groups.iter().map(|group| library_group(group, cx)))
                .into_any_element()
        }
        SectionData::LocalAnimeLibrary(media) => {
            if media.is_empty() {
                return empty("No matched media on disk yet.");
            }
            carousels.read(cx).local_grid.clone().into_any_element()
        }
        SectionData::LocalAnimeLibraryStats(stats) => stats_strip(stats, cx),
        SectionData::LibraryUpcomingEpisodes(episodes) => {
            if episodes.is_empty() {
                return empty("Nothing upcoming for your library media.");
            }
            div()
                .flex()
                .gap_3()
                .children(episodes.iter().map(|episode| {
                    MediaCard::new(
                        MediaCardView {
                            detail: format!(
                                "Episode {} · {}",
                                episode.episode,
                                countdown(episode.time_until_airing)
                            )
                            .into(),
                            ..media_card(&episode.media)
                        },
                        SharedString::from(format!("home-upcoming-{}", episode.media.anilist_id)),
                    )
                    .into_any_element()
                }))
                .into_any_element()
        }
        SectionData::CenteredTitle(title) => div()
            .w_full()
            .flex()
            .justify_center()
            .text_2xl()
            .text_color(colors.muted_foreground)
            .child(title.clone())
            .into_any_element(),
        SectionData::MyLists(_)
        | SectionData::MissedSequels(_)
        | SectionData::AiredRecently(_)
        | SectionData::AnimeCarousel(_) => {
            let (cards, ids) = section_data_cards(data);
            match carousels.read(cx).slot(section) {
                Some((state, wheel)) if !cards.is_empty() => MediaCarousel::new(
                    &format!("{}-carousel", section_id(section)),
                    state,
                    cards,
                    cache.clone(),
                    wheel,
                )
                .on_open(open_entry(ids, cx))
                .into_any_element(),
                _ => empty("Nothing to show yet."),
            }
        }
    }
}

/// One `EpisodeCard` that plays the next file when clicked.
fn continue_card(
    item: &ContinueEpisode,
    index: usize,
    playback_store: &Entity<PlaybackStore>,
) -> gpui_kit::AnyElement {
    let media = item.entry.media.as_ref();
    let view = EpisodeCardView {
        thumbnail: media
            .and_then(|media| media.banner.clone().or_else(|| media.poster.clone()))
            .map(|url| Resource::Uri(url.into())),
        episode_number: item.number,
        title: item.label.clone().into(),
        media_title: media.map(|media| title_of(media).into()),
        filename: Some(item.path.to_string_lossy().into_owned().into()),
        filler: false,
        watched: false,
        locked: false,
        progress: item.progress.map(|percent| f32::from(percent) / 100.0),
        skeleton: false,
        unavailable: None,
        selected: false,
    };
    let request = item.play_request();
    let store = playback_store.clone();
    EpisodeCard::new(
        view,
        SharedString::from(format!("home-continue-episode-{index}")),
    )
    .actions(EpisodeActions::new().on_play(move |_, cx| {
        store.update(cx, |store, cx| store.play(request.clone(), cx));
    }))
    .into_any_element()
}

/// One list-status group of poster cards.
fn library_group(group: &LibraryGroup, cx: &mut Context<'_, AppShell>) -> gpui_kit::AnyElement {
    let label = match group.status {
        domain::ListStatus::Current | domain::ListStatus::Repeating => "Watching",
        domain::ListStatus::Paused => "Paused",
        domain::ListStatus::Planning => "Planning",
        domain::ListStatus::Completed => "Completed",
        domain::ListStatus::Dropped => "Dropped",
        _ => "Other",
    };
    div()
        .flex()
        .flex_col()
        .gap_2()
        .child(
            div()
                .text_sm()
                .text_color(cx.theme().colors.muted_foreground)
                .child(label),
        )
        .child(
            div()
                .flex()
                .gap_3()
                .children(group.entries.iter().map(|item| {
                    let mut view = match item.media.as_ref() {
                        Some(media) => media_card(media),
                        None => MediaCardView {
                            title: entry_title(item).into(),
                            ..MediaCardView::default()
                        },
                    };
                    view.progress = item.entry.progress;
                    let id = item.media.as_ref().map(|media| media.anilist_id);
                    let shell = cx.entity().downgrade();
                    MediaCard::new(
                        view,
                        SharedString::from(format!("home-library-{}", item.entry.media.get())),
                    )
                    .on_open(move |_, cx| {
                        if let Some(id) = id {
                            let _ = shell.update(cx, |shell, cx| {
                                shell.navigate(Route::Entry { id, tab: None }, cx);
                            });
                        }
                    })
                    .into_any_element()
                })),
        )
        .into_any_element()
}

/// The four counts, with a link to the scan log.
fn stats_strip(stats: &LibraryStats, cx: &mut Context<'_, AppShell>) -> gpui_kit::AnyElement {
    let colors = cx.theme().colors;
    let shell = cx.entity().downgrade();
    let cell = |label: &'static str, value: String| {
        div()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child(label),
            )
            .child(div().text_lg().child(value))
            .into_any_element()
    };
    div()
        .flex()
        .gap_6()
        .child(cell("Files", stats.files.to_string()))
        .child(cell("Size", format!("{} MB", stats.bytes / (1024 * 1024))))
        .child(cell("Media", stats.media.to_string()))
        .child(cell("Unmatched", stats.unmatched.to_string()))
        .child(
            Button::new("home-stats-scan-summaries")
                .label("Scan Summaries")
                .on_click(move |_, _, cx| {
                    let _ = shell.update(cx, |shell, cx| shell.navigate(Route::ScanSummaries, cx));
                }),
        )
        .into_any_element()
}

/// A short relative time for an upcoming episode's countdown.
fn countdown(seconds: i32) -> String {
    if seconds <= 0 {
        return "aired".to_owned();
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("in {minutes}m");
    }
    let hours = minutes / 60;
    if hours < 24 {
        return format!("in {hours}h");
    }
    format!("in {}d", hours / 24)
}

/// A hero with one action that opens the media's Entry.
fn hero(
    id: &'static str,
    data: HeroBannerData,
    anilist_id: Option<u32>,
    action: &'static str,
    cache: &Entity<LruImageCache>,
    cx: &mut Context<'_, AppShell>,
) -> gpui_kit::AnyElement {
    let colors = cx.theme().colors;
    let mut button = Button::new(SharedString::from(format!("{id}-open"))).label(action);
    if let Some(anilist_id) = anilist_id {
        let open = open_entry(vec![Some(anilist_id)], cx);
        button = button.on_click(move |_, window, cx| open(0, window, cx));
    }
    hero_banner(
        id,
        data,
        cache,
        colors.accent,
        colors.background,
        div().child(button),
    )
    .into_any_element()
}

fn hero_data(item: &HomeEntry) -> HeroBannerData {
    let total = item
        .media
        .as_ref()
        .and_then(|media| media.episodes)
        .map_or_else(String::new, |total| format!(" of {total}"));
    HeroBannerData {
        title: entry_title(item).into(),
        synopsis: format!("Episode {}{total}", item.entry.progress).into(),
        banner: item.media.as_ref().and_then(|media| media.banner.clone()),
    }
}

fn entry_title(item: &HomeEntry) -> String {
    item.media
        .as_ref()
        .map_or_else(|| "Untitled".to_owned(), title_of)
}

fn empty(message: &'static str) -> gpui_kit::AnyElement {
    div()
        .h(px(MEDIA_CARD_HEIGHT / 2.))
        .child(message)
        .into_any_element()
}

/// The AniList id behind each card, `None` for a media with no cached summary.
type CardIds = Vec<Option<u32>>;

/// The cards of a section and the AniList id behind each one.
fn section_cards(state: &SectionState) -> (Rc<[MediaCardView]>, CardIds) {
    match state {
        SectionState::Ready(data) | SectionState::Offline(data, _) => section_data_cards(data),
        SectionState::Loading | SectionState::Hidden | SectionState::Failed(_) => {
            (Rc::from([]), Vec::new())
        }
    }
}

fn section_data_cards(data: &SectionData) -> (Rc<[MediaCardView]>, CardIds) {
    let media_cards = |media: &mut dyn Iterator<Item = &MediaSummary>| {
        let media: Vec<&MediaSummary> = media.collect();
        (
            media.iter().map(|item| media_card(item)).collect(),
            media.iter().map(|item| Some(item.anilist_id)).collect(),
        )
    };
    match data {
        SectionData::MyLists(entries) => {
            let cards = entries
                .iter()
                .map(|item| match &item.media {
                    Some(media) => MediaCardView {
                        progress: item.entry.progress,
                        ..media_card(media)
                    },
                    // Never opened, so nothing is cached: a text-only card.
                    None => MediaCardView {
                        poster: None,
                        title: entry_title(item).into(),
                        format: None,
                        score: None,
                        progress: item.entry.progress,
                        total: None,
                        detail: SharedString::default(),
                    },
                })
                .collect();
            let ids = entries
                .iter()
                .map(|item| item.media.as_ref().map(|media| media.anilist_id))
                .collect();
            (cards, ids)
        }
        SectionData::MissedSequels(media) | SectionData::AnimeCarousel(media) => {
            media_cards(&mut media.iter())
        }
        SectionData::AiredRecently(episodes) => {
            let cards = episodes
                .iter()
                .map(|episode| MediaCardView {
                    detail: format!("Episode {}", episode.episode).into(),
                    ..media_card(&episode.media)
                })
                .collect();
            let ids = episodes
                .iter()
                .map(|episode| Some(episode.media.anilist_id))
                .collect();
            (cards, ids)
        }
        SectionData::ContinueWatching(_)
        | SectionData::AnimeContinueWatching(_)
        | SectionData::DiscoverHeader(_)
        | SectionData::AnimeLibrary(_)
        | SectionData::LocalAnimeLibrary(_)
        | SectionData::LocalAnimeLibraryStats(_)
        | SectionData::LibraryUpcomingEpisodes(_)
        | SectionData::CenteredTitle(_) => (Rc::from([]), Vec::new()),
    }
}

/// Opens the Entry of the media at a card's index.
fn open_entry(
    ids: CardIds,
    cx: &mut Context<'_, AppShell>,
) -> impl Fn(usize, &mut Window, &mut App) + 'static {
    let shell = cx.entity().downgrade();
    move |index, _, cx| {
        if let Some(Some(id)) = ids.get(index).copied() {
            let _ = shell.update(cx, |shell, cx| {
                shell.navigate(Route::Entry { id, tab: None }, cx);
            });
        }
    }
}

/// The element id of a section, for tests and the carousel's child ids.
pub(crate) fn section_id(section: HomeSection) -> &'static str {
    match section {
        HomeSection::ContinueWatching => "home-continue-watching-header",
        HomeSection::AnimeContinueWatching => "anime-continue-watching",
        HomeSection::DiscoverHeader => "home-discover-header",
        HomeSection::MyLists => "home-my-lists",
        HomeSection::AnimeLibrary => "anime-library",
        HomeSection::LocalAnimeLibrary => "local-anime-library",
        HomeSection::LocalAnimeLibraryStats => "local-anime-library-stats",
        HomeSection::LibraryUpcomingEpisodes => "library-upcoming-episodes",
        HomeSection::MissedSequels => "home-missed-sequels",
        HomeSection::AiredRecently => "home-aired-recently",
        HomeSection::AnimeCarousel => "home-anime-carousel",
        HomeSection::CenteredTitle => "home-centered-title",
    }
}

/// The heading above a carousel section; the headers and the title have none.
fn section_title(section: HomeSection) -> Option<&'static str> {
    match section {
        HomeSection::AnimeContinueWatching => Some("Continue Watching"),
        HomeSection::MyLists => Some("My Lists"),
        HomeSection::AnimeLibrary => Some("Library"),
        HomeSection::LocalAnimeLibrary => Some("Local Library"),
        HomeSection::LocalAnimeLibraryStats => Some("Library Stats"),
        HomeSection::LibraryUpcomingEpisodes => Some("Upcoming Episodes"),
        HomeSection::MissedSequels => Some("Missed Sequels"),
        HomeSection::AiredRecently => Some("Aired Recently"),
        HomeSection::AnimeCarousel => Some("Popular Anime"),
        HomeSection::ContinueWatching
        | HomeSection::DiscoverHeader
        | HomeSection::CenteredTitle => None,
    }
}

#[cfg(test)]
mod tests {
    use std::{rc::Rc, sync::Arc, time::Duration, time::SystemTime};

    use db::{LocalFile, ResumePoint};
    use domain::{
        AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score, StreamSource,
    };
    use gpui_kit::{
        App, AppContext, Bounds, Task, TestAppContext, WindowBounds, WindowOptions, base::Root,
        point, px, size, test::TestWindowExt,
    };
    use metadata::{AiringEpisode, MediaDetail, MediaSummary, MediaTitle};
    use tokio::runtime::Builder;
    use ui_state::{
        LibraryMediaInfo, Route,
        bridge::{self, BridgeError},
        entry_store::EntryStore,
        home_store::{HomeSection, HomeStore},
        playback_store::PlaybackStore,
        test_support::{CannedServices, PlaybackCall, PlaybackServices},
    };

    use crate::{
        discover::DiscoverUpdate,
        shell::{AppShell, RailPrefs},
    };

    use super::section_id;

    const WATCHED: u32 = 100;

    fn summary() -> MediaSummary {
        MediaSummary {
            anilist_id: WATCHED,
            mal_id: None,
            kind: Some(MediaKind::Anime),
            title: MediaTitle {
                romaji: Some("A Watched Show".to_owned()),
                ..MediaTitle::default()
            },
            poster: None,
            banner: None,
            format: None,
            status: None,
            season: None,
            season_year: None,
            episodes: Some(12),
            chapters: None,
            mean_score: None,
            genres: Vec::new(),
            is_adult: false,
        }
    }

    fn detail() -> MediaDetail {
        MediaDetail {
            summary: summary(),
            description: None,
            start_date: FuzzyDate::UNKNOWN,
            end_date: FuzzyDate::UNKNOWN,
            duration: None,
            studios: Vec::new(),
            popularity: None,
            characters: Vec::new(),
            relations: Vec::new(),
            recommendations: Vec::new(),
        }
    }

    fn watching() -> ListEntry {
        ListEntry {
            media: MediaId::new(1),
            status: ListStatus::Current,
            score: Score::UNSCORED,
            progress: 3,
            progress_volumes: 0,
            repeat: 0,
            started_at: FuzzyDate::UNKNOWN,
            completed_at: FuzzyDate::UNKNOWN,
            notes: String::new(),
            private: false,
            priority: 0,
            advanced_scores: AdvancedScores::default(),
            custom_lists: Vec::new(),
            hidden_from_status_lists: false,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    #[gpui_kit::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one screen test builds the shell, then checks order, isolation and navigation"
    )]
    async fn home_renders_the_widgets_in_order_and_isolates_a_failed_source(
        cx: &mut TestAppContext,
    ) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));

        // The catalog fails; the Local list holds one show being watched.
        let services = Arc::new(
            CannedServices::new(detail())
                .home_fails()
                .collection(vec![(watching(), Some(summary()))]),
        );
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                cx.set_reduce_motion(true);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(4000.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    let entry_store =
                        cx.new(|_| EntryStore::new(Arc::<CannedServices>::clone(&services)));
                    let home_store =
                        cx.new(|_| HomeStore::new(Arc::<CannedServices>::clone(&services)));
                    cx.new(|cx| {
                        AppShell::new(
                            RailPrefs::default(),
                            settings::PlaybackSettings::default(),
                            crate::shell::PlaybackServices::inert(),
                            settings::NetworkSettings::default(),
                            crate::shell::NetworkServices::inert(),
                            settings::LibrarySettings::default(),
                            crate::shell::LibraryServices::inert(),
                            cx.new(|_| {
                                ui_state::LibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| {
                                ui_state::ScanSummariesStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| {
                                ui_state::LibraryExplorerStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| ui_state::fetch_status::FetchStatus::default()),
                            cx.new(|_| ui_state::account::AccountStore::default()),
                            "https://example.test/login".to_owned(),
                            |_, _| Task::ready(()),
                            |_, _| Task::ready(Ok(0)),
                            |_, _| Task::ready(()),
                            Rc::new(|_: Option<String>, _: &App| {
                                Task::ready(Ok::<_, BridgeError>(DiscoverUpdate::default()))
                            }),
                            entry_store,
                            cx.new(|_| {
                                ui_state::EntryLibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            home_store,
                            cx.new(|_| {
                                ui_state::PlaybackStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            settings::GeneralSettings {
                                onboarding_completed: true,
                            },
                            crate::shell::GeneralServices::inert(),
                            true,
                            window,
                            cx,
                        )
                    })
                })
            })
            .expect("a test window");
        let window = window.downcast::<Root>().expect("the root window");
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            let tops: Vec<_> = HomeSection::ALL
                .iter()
                .map(|section| window.find(section_id(*section)).bounds().origin.y)
                .collect();
            assert!(
                tops.windows(2).all(|pair| pair.first() < pair.get(1)),
                "the seven widgets stack in the fixed order: {tops:?}"
            );
            for section in [
                HomeSection::DiscoverHeader,
                HomeSection::AiredRecently,
                HomeSection::AnimeCarousel,
            ] {
                assert!(
                    window
                        .try_find(gpui_kit::SharedString::from(format!(
                            "{}-error",
                            section_id(section)
                        )))
                        .is_some(),
                    "{section:?} shows its own error card"
                );
            }
            for section in [
                HomeSection::ContinueWatching,
                HomeSection::MyLists,
                HomeSection::MissedSequels,
            ] {
                assert!(
                    window
                        .try_find(gpui_kit::SharedString::from(format!(
                            "{}-error",
                            section_id(section)
                        )))
                        .is_none(),
                    "{section:?} is unaffected by the catalog failure"
                );
            }
            // The show on the Local list leads Home, and opens its Entry.
            window.click("home-continue-open", cx);
        })
        .expect("the rendered Home");
        cx.run_until_parked();
        assert_eq!(
            view.read_with(cx, |shell, _| shell.route()),
            Route::Entry {
                id: WATCHED,
                tab: None
            },
            "Continue opens the watched show's Entry"
        );
    }

    #[gpui_kit::test]
    #[expect(
        clippy::too_many_lines,
        reason = "one screen test builds the shell, then checks the toolbar, widgets and play action"
    )]
    async fn library_widgets_render_and_their_episode_cards_play(cx: &mut TestAppContext) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let file = LocalFile {
            path: "/shows/ep3.mkv".into(),
            size: 1024 * 1024,
            modified: 0,
            title: None,
            group: None,
            season: None,
            episode: Some(3),
            kind: "episode".to_owned(),
            media_id: Some(MediaId::new(1)),
            confidence: Some(0.9),
            locked: false,
            ignored: false,
            filler: false,
            shelved: false,
        };
        let services = Arc::new(
            CannedServices::new(detail())
                .collection(vec![(watching(), Some(summary()))])
                .library(
                    vec![file],
                    vec![LibraryMediaInfo::new(
                        MediaId::new(1),
                        Some(WATCHED),
                        Some("A Watched Show".to_owned()),
                    )],
                    vec![ResumePoint {
                        media: MediaId::new(1),
                        episode: 3,
                        position: 600.0,
                        duration: Some(1200.0),
                        updated_at: 100,
                    }],
                    vec![AiringEpisode {
                        media: summary(),
                        episode: 4,
                        airing_at: 0,
                        time_until_airing: 3600,
                    }],
                ),
        );
        let playback = PlaybackServices::default();
        let playback_calls = playback.clone();
        let (window, _) = cx
            .update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                cx.set_reduce_motion(true);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(4000.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    let entry_store =
                        cx.new(|_| EntryStore::new(Arc::<CannedServices>::clone(&services)));
                    let home_store =
                        cx.new(|_| HomeStore::new(Arc::<CannedServices>::clone(&services)));
                    cx.new(|cx| {
                        AppShell::new(
                            RailPrefs::default(),
                            settings::PlaybackSettings::default(),
                            crate::shell::PlaybackServices::inert(),
                            settings::NetworkSettings::default(),
                            crate::shell::NetworkServices::inert(),
                            settings::LibrarySettings::default(),
                            crate::shell::LibraryServices::inert(),
                            cx.new(|_| {
                                ui_state::LibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| {
                                ui_state::ScanSummariesStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| {
                                ui_state::LibraryExplorerStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| ui_state::fetch_status::FetchStatus::default()),
                            cx.new(|_| ui_state::account::AccountStore::default()),
                            "https://example.test/login".to_owned(),
                            |_, _| Task::ready(()),
                            |_, _| Task::ready(Ok(0)),
                            |_, _| Task::ready(()),
                            Rc::new(|_: Option<String>, _: &App| {
                                Task::ready(Ok::<_, BridgeError>(DiscoverUpdate::default()))
                            }),
                            entry_store,
                            cx.new(|_| {
                                ui_state::EntryLibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            home_store,
                            cx.new(|_| PlaybackStore::new(Arc::new(playback.clone()))),
                            settings::GeneralSettings {
                                onboarding_completed: true,
                            },
                            crate::shell::GeneralServices::inert(),
                            true,
                            window,
                            cx,
                        )
                    })
                })
            })
            .expect("a test window");
        let window = window.downcast::<Root>().expect("the root window");
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            for section in [
                HomeSection::AnimeContinueWatching,
                HomeSection::AnimeLibrary,
                HomeSection::LocalAnimeLibrary,
                HomeSection::LocalAnimeLibraryStats,
                HomeSection::LibraryUpcomingEpisodes,
            ] {
                assert!(
                    window.find(section_id(section)).bounds().size.height > px(0.),
                    "{section:?} is on Home"
                );
            }
            assert!(
                window.try_find("home-scan-library").is_some(),
                "the toolbar offers a manual scan"
            );
            window.click("home-continue-episode-0", cx);
        })
        .expect("the rendered Home");
        cx.run_until_parked();
        let calls = playback_calls.calls();
        assert!(
            calls.iter().any(|call| matches!(
                call,
                PlaybackCall::Play(request)
                    if request.episode == 3
                        && request.source == StreamSource::local("/shows/ep3.mkv")
            )),
            "clicking the episode card plays its local file: {calls:?}"
        );
    }
}
