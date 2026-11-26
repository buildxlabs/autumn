//! The Home screen's fixed widget set: the Home store (#61, #136).
//!
//! The store holds every section's state, and each section resolves on its
//! own, so one failing section doesn't stop the others. The platform
//! sections come from one collection read and one missed-sequels read; the
//! catalog sections come from one stale-first Home load: the cached value
//! shows at once, and a refresh that can't reach AniList marks the sections
//! `Offline` over the cached data. The library sections read the local file
//! table, saved resume positions and the airing schedule; they make no other
//! AniList request (ARCHITECTURE §6).

use std::{
    collections::{HashMap, HashSet},
    path::PathBuf,
    sync::Arc,
    time::SystemTime,
};

use db::{LocalFile, ResumePoint};
use domain::{ListEntry, ListStatus, MediaId, MediaKind};
use engine::{EngineServices, LibraryMediaInfo};
use gpui_kit::{Context, Task, WeakEntity};
use metadata::{AiringEpisode, Cached, HomeLoad, Load, MediaSummary};

use crate::{bridge, entry_store::call};

/// The Home screen's fixed widget set, in order (#23, the Phase 1 exit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Home screen matches every section, so a new section must not compile until it has one"
)]
pub enum HomeSection {
    /// The continue-watching header: the shows the user is watching.
    ContinueWatching,
    /// The library's next episodes, with a resume position first.
    AnimeContinueWatching,
    /// The discover header: the catalog's trending anime.
    DiscoverHeader,
    /// The user's list.
    MyLists,
    /// The user's media with local files, grouped by list status.
    AnimeLibrary,
    /// Every matched media on disk.
    LocalAnimeLibrary,
    /// Counts for the local library.
    LocalAnimeLibraryStats,
    /// Airing episodes of the media in the local library.
    LibraryUpcomingEpisodes,
    /// Sequels of completed shows that are not on the list yet.
    MissedSequels,
    /// The aired-recently carousel.
    AiredRecently,
    /// The anime carousel.
    AnimeCarousel,
    /// The centered title.
    CenteredTitle,
}

impl HomeSection {
    /// Every section, in the order they render.
    pub const ALL: [Self; 12] = [
        Self::ContinueWatching,
        Self::AnimeContinueWatching,
        Self::DiscoverHeader,
        Self::MyLists,
        Self::AnimeLibrary,
        Self::LocalAnimeLibrary,
        Self::LocalAnimeLibraryStats,
        Self::LibraryUpcomingEpisodes,
        Self::MissedSequels,
        Self::AiredRecently,
        Self::AnimeCarousel,
        Self::CenteredTitle,
    ];

    /// The index of the section in [`Self::ALL`].
    fn index(self) -> usize {
        match self {
            Self::ContinueWatching => 0,
            Self::AnimeContinueWatching => 1,
            Self::DiscoverHeader => 2,
            Self::MyLists => 3,
            Self::AnimeLibrary => 4,
            Self::LocalAnimeLibrary => 5,
            Self::LocalAnimeLibraryStats => 6,
            Self::LibraryUpcomingEpisodes => 7,
            Self::MissedSequels => 8,
            Self::AiredRecently => 9,
            Self::AnimeCarousel => 10,
            Self::CenteredTitle => 11,
        }
    }

    /// The sections filled from the active platform.
    const PLATFORM: [Self; 3] = [Self::ContinueWatching, Self::MyLists, Self::MissedSequels];

    /// The sections filled from the Home catalog load.
    const CATALOG: [Self; 3] = [
        Self::DiscoverHeader,
        Self::AiredRecently,
        Self::AnimeCarousel,
    ];

    /// The sections filled from the local library (#136). They hide together
    /// when no files have been scanned.
    const LIBRARY: [Self; 5] = [
        Self::AnimeContinueWatching,
        Self::AnimeLibrary,
        Self::LocalAnimeLibrary,
        Self::LocalAnimeLibraryStats,
        Self::LibraryUpcomingEpisodes,
    ];
}

/// The state of one Home widget.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Home renderer matches every section state"
)]
pub enum SectionState {
    /// The section's data is loading.
    Loading,
    /// A library widget with no local files yet: the renderer skips it.
    Hidden,
    /// The section's data.
    Ready(SectionData),
    /// The request failed; the screen shows the section's error card.
    Failed(String),
    /// Cached data a refresh couldn't update because AniList is out of
    /// reach, with when it was last updated; the screen shows the offline
    /// banner over it.
    Offline(SectionData, SystemTime),
}

/// A list entry with the card text the app already has on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeEntry {
    /// The list entry.
    pub entry: ListEntry,
    /// The cached summary: `None` when the media was never opened.
    pub media: Option<MediaSummary>,
}

/// The next episode of one media with a local file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContinueEpisode {
    /// The list entry and its cached card text.
    pub entry: HomeEntry,
    /// The episode to play.
    pub episode: u32,
    /// The local file that plays it.
    pub path: PathBuf,
    /// Saved position as a whole percentage, when the episode was resumed.
    pub progress: Option<u8>,
}

/// The media with local files in one list status.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryGroup {
    /// The status the group collects.
    pub status: ListStatus,
    /// Its entries, most recently updated first.
    pub entries: Vec<HomeEntry>,
}

/// The counts shown in the library's stats strip.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LibraryStats {
    /// Local files on disk.
    pub files: usize,
    /// Bytes those files occupy.
    pub bytes: u64,
    /// Distinct matched media.
    pub media: usize,
    /// Files with no media match.
    pub unmatched: usize,
}

/// One Home widget's data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "a section renders exactly one data variant"
)]
pub enum SectionData {
    /// The continue-watching header: Watching entries, most recent first.
    ContinueWatching(Vec<HomeEntry>),
    /// The library's next episodes, resumed ones first.
    AnimeContinueWatching(Vec<ContinueEpisode>),
    /// The discover header: trending anime.
    DiscoverHeader(Vec<MediaSummary>),
    /// The user's list, most recently updated first.
    MyLists(Vec<HomeEntry>),
    /// The user's local media, grouped by list status.
    AnimeLibrary(Vec<LibraryGroup>),
    /// Every matched media on disk.
    LocalAnimeLibrary(Vec<LibraryMediaInfo>),
    /// Counts for the local library.
    LocalAnimeLibraryStats(LibraryStats),
    /// Airing episodes of the media in the local library.
    LibraryUpcomingEpisodes(Vec<AiringEpisode>),
    /// The missed sequels.
    MissedSequels(Vec<MediaSummary>),
    /// The aired-recently carousel.
    AiredRecently(Vec<AiringEpisode>),
    /// The anime carousel.
    AnimeCarousel(Vec<MediaSummary>),
    /// The centered title.
    CenteredTitle(String),
}

/// The app name shown in the centered-title widget.
const TITLE: &str = "Autumn";

/// The Home store: the fixed Home widget set and each section's state.
pub struct HomeStore {
    services: Arc<dyn EngineServices>,
    /// Each section's state, in [`HomeSection::ALL`] order.
    states: Vec<SectionState>,
    /// The platform sections' load; replacing it cancels the older one.
    platform_task: Option<Task<()>>,
    /// The catalog sections' load and refresh; replacing it cancels both.
    catalog_task: Option<Task<()>>,
    /// The library sections' load; replacing it cancels the older one.
    library_task: Option<Task<()>>,
    /// The collection behind the platform and library sections, kept so a
    /// `LibraryChanged` event can refresh them without another read.
    collection: Vec<HomeEntry>,
}

impl std::fmt::Debug for HomeStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HomeStore")
            .field("states", &self.states)
            .finish_non_exhaustive()
    }
}

impl HomeStore {
    /// A store with every section loading.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        let mut states = HomeSection::ALL.map(|_| SectionState::Loading).to_vec();
        if let Some(slot) = states.get_mut(HomeSection::CenteredTitle.index()) {
            *slot = SectionState::Ready(SectionData::CenteredTitle(TITLE.to_owned()));
        }
        Self {
            services,
            states,
            platform_task: None,
            catalog_task: None,
            library_task: None,
            collection: Vec::new(),
        }
    }

    /// The state of every section, in widget order.
    #[must_use]
    pub fn states(&self) -> &[SectionState] {
        &self.states
    }

    /// The state of one section.
    #[expect(
        clippy::indexing_slicing,
        reason = "states is built with one entry per section, in ALL order"
    )]
    #[must_use]
    pub fn state(&self, section: HomeSection) -> &SectionState {
        &self.states[section.index()]
    }

    /// Loads every section. Sections already on screen stay there until
    /// the new value lands, so returning to Home doesn't flash loaders.
    pub fn open(&mut self, cx: &mut Context<'_, Self>) {
        self.load_platform(cx);
        self.load_catalog(cx);
    }

    /// Reloads the list sections, as after a list entry was saved.
    pub fn reload_lists(&mut self, cx: &mut Context<'_, Self>) {
        self.load_platform(cx);
    }

    /// Reloads the sections that share `section`'s source, showing their
    /// loaders first.
    pub fn retry(&mut self, section: HomeSection, cx: &mut Context<'_, Self>) {
        if HomeSection::PLATFORM.contains(&section) {
            self.set_loading(&HomeSection::PLATFORM);
            self.load_platform(cx);
        } else if HomeSection::CATALOG.contains(&section) {
            self.set_loading(&HomeSection::CATALOG);
            self.load_catalog(cx);
        } else if HomeSection::LIBRARY.contains(&section) {
            self.set_loading(&HomeSection::LIBRARY);
            self.load_library(cx);
        }
        cx.notify();
    }

    /// Applies an engine event. A library change refreshes the library
    /// widgets; the scan's live progress stays with [`crate::LibraryStore`].
    pub fn apply(&mut self, event: &domain::AppEvent, cx: &mut Context<'_, Self>) {
        if matches!(event, domain::AppEvent::LibraryChanged) {
            self.library_task = None;
            self.load_library(cx);
            cx.notify();
        }
    }

    fn set_loading(&mut self, sections: &[HomeSection]) {
        for section in sections {
            self.set(*section, SectionState::Loading);
        }
    }

    fn set(&mut self, section: HomeSection, state: SectionState) {
        if let Some(slot) = self.states.get_mut(section.index()) {
            *slot = state;
        }
    }

    /// The platform sections: one collection read fills continue-watching
    /// and my lists; missed sequels has its own read, so either can fail
    /// alone.
    fn load_platform(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.platform_task = Some(cx.spawn(async move |store, cx| {
            let collection = call(cx, services.collection(MediaKind::Anime)).await;
            let (watching, lists, entries) = match collection {
                Ok(listed) => {
                    let entries: Vec<HomeEntry> = listed
                        .into_iter()
                        .map(|(entry, media)| HomeEntry { entry, media })
                        .collect();
                    let watching = entries
                        .iter()
                        .filter(|item| item.entry.status == ListStatus::Current)
                        .cloned()
                        .collect();
                    (
                        SectionState::Ready(SectionData::ContinueWatching(watching)),
                        SectionState::Ready(SectionData::MyLists(entries.clone())),
                        entries,
                    )
                }
                Err(message) => (
                    SectionState::Failed(message.clone()),
                    SectionState::Failed(message),
                    Vec::new(),
                ),
            };
            update(&store, cx, |store| {
                store.collection = entries;
                store.set(HomeSection::ContinueWatching, watching);
                store.set(HomeSection::MyLists, lists);
            });
            // One collection read fills both the platform and the library
            // sections; the library's own calls carry their own failures.
            let _ = store.update(cx, |store, cx| {
                store.load_library(cx);
                cx.notify();
            });
            let missed = match call(cx, services.missed_sequels(MediaKind::Anime)).await {
                Ok(sequels) => SectionState::Ready(SectionData::MissedSequels(sequels)),
                Err(message) => SectionState::Failed(message),
            };
            update(&store, cx, |store| {
                store.set(HomeSection::MissedSequels, missed);
            });
        }));
    }

    /// The catalog sections: one stale-first Home load, then its refresh
    /// when the cached value is stale.
    fn load_catalog(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.catalog_task = Some(cx.spawn(async move |store, cx| {
            let load = match call(cx, services.home()).await {
                Ok(load) => load,
                Err(message) => {
                    update(&store, cx, |store| {
                        for section in HomeSection::CATALOG {
                            store.set(section, SectionState::Failed(message.clone()));
                        }
                    });
                    return;
                }
            };
            update(&store, cx, |store| {
                // Offline sections stay offline while the refresh that may
                // fail again is pending, so the banner doesn't flicker.
                let offline = matches!(
                    store.state(HomeSection::DiscoverHeader),
                    SectionState::Offline(..)
                );
                if !(offline && load.refresh.is_some()) {
                    store.apply_catalog(&load.cached);
                }
            });
            let Some(refresh) = load.refresh else {
                return;
            };
            match cx.update(|cx| bridge::run(cx, refresh)).await {
                Ok(Ok(fresh)) => update(&store, cx, |store| store.apply_catalog(&fresh)),
                // AniList answered with an error: the cached value stays.
                Ok(Err(error)) => tracing::warn!(%error, "the Home catalog refresh failed"),
                Err(error) => tracing::warn!(%error, "the Home catalog refresh was dropped"),
            }
        }));
    }

    /// Reads the library from the engine. The four calls are independent:
    /// each widget's state is set from the result it needs, so one failed
    /// call doesn't blank the others.
    fn load_library(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        let entries = self.collection.clone();
        self.library_task = Some(cx.spawn(async move |store, cx| {
            let files = call(cx, services.all_library_files()).await;
            let info = call(cx, services.library_media_info()).await;
            let resume = call(cx, services.resume_points()).await;
            let schedule = call(cx, services.airing_schedule()).await;
            update(&store, cx, |store| {
                store.apply_library(&entries, files, info, resume, schedule);
            });
        }));
    }

    fn apply_library(
        &mut self,
        entries: &[HomeEntry],
        files: Result<Vec<LocalFile>, String>,
        info: Result<Vec<LibraryMediaInfo>, String>,
        resume: Result<Vec<ResumePoint>, String>,
        schedule: Result<Load<Vec<AiringEpisode>>, String>,
    ) {
        // Nothing scanned: the five library widgets are not part of Home yet.
        if files.as_ref().is_ok_and(Vec::is_empty) {
            for section in HomeSection::LIBRARY {
                self.set(section, SectionState::Hidden);
            }
            return;
        }
        let files_error = files.as_ref().err().cloned();
        let info_error = info.as_ref().err().cloned();
        let resume_error = resume.as_ref().err().cloned();
        let schedule_error = schedule.as_ref().err().cloned();
        let files = files.ok();
        let info = info.ok();
        let resume = resume.ok();
        let schedule = schedule.ok();

        if let Some(message) = &files_error {
            self.fail(HomeSection::AnimeContinueWatching, message);
            self.fail(HomeSection::AnimeLibrary, message);
            self.fail(HomeSection::LocalAnimeLibraryStats, message);
        }
        if let Some(message) = &resume_error {
            self.fail(HomeSection::AnimeContinueWatching, message);
        }
        if let Some(message) = &info_error {
            self.fail(HomeSection::LocalAnimeLibrary, message);
            self.fail(HomeSection::LibraryUpcomingEpisodes, message);
        }
        if let Some(message) = &schedule_error {
            self.fail(HomeSection::LibraryUpcomingEpisodes, message);
        }

        if let Some(files) = files.as_ref() {
            let matched: std::collections::HashSet<MediaId> =
                files.iter().filter_map(|file| file.media_id).collect();
            if self.pending(HomeSection::AnimeContinueWatching) && resume.is_some() {
                let episodes = next_episodes(entries, files, resume.as_deref().unwrap_or(&[]));
                self.set(
                    HomeSection::AnimeContinueWatching,
                    SectionState::Ready(SectionData::AnimeContinueWatching(episodes)),
                );
            }
            if self.pending(HomeSection::AnimeLibrary) {
                self.set(
                    HomeSection::AnimeLibrary,
                    SectionState::Ready(SectionData::AnimeLibrary(status_groups(
                        entries, &matched,
                    ))),
                );
            }
            if self.pending(HomeSection::LocalAnimeLibraryStats) {
                self.set(
                    HomeSection::LocalAnimeLibraryStats,
                    SectionState::Ready(SectionData::LocalAnimeLibraryStats(stats(files))),
                );
            }
        }
        let Some(info) = info.as_ref() else {
            return;
        };
        if self.pending(HomeSection::LocalAnimeLibrary) {
            self.set(
                HomeSection::LocalAnimeLibrary,
                SectionState::Ready(SectionData::LocalAnimeLibrary(info.clone())),
            );
        }
        if self.pending(HomeSection::LibraryUpcomingEpisodes)
            && let Some(schedule) = schedule
        {
            let cached = schedule.cached;
            let ids: std::collections::HashSet<u32> =
                info.iter().filter_map(|media| media.anilist_id).collect();
            let mut upcoming: Vec<AiringEpisode> = cached
                .value
                .into_iter()
                .filter(|episode| ids.contains(&episode.media.anilist_id))
                .collect();
            upcoming.sort_by_key(|episode| episode.time_until_airing);
            let data = SectionData::LibraryUpcomingEpisodes(upcoming);
            let state = if cached.offline {
                SectionState::Offline(data, cached.updated_at)
            } else {
                SectionState::Ready(data)
            };
            self.set(HomeSection::LibraryUpcomingEpisodes, state);
        }
    }

    fn pending(&self, section: HomeSection) -> bool {
        matches!(self.state(section), SectionState::Loading)
    }

    fn fail(&mut self, section: HomeSection, message: &str) {
        self.set(section, SectionState::Failed(message.to_owned()));
    }

    /// Shows a Home load in the catalog sections: offline when the refresh
    /// behind it couldn't reach AniList.
    fn apply_catalog(&mut self, home: &Cached<HomeLoad>) {
        let value = &home.value;
        for (section, data) in [
            (
                HomeSection::DiscoverHeader,
                SectionData::DiscoverHeader(value.trending.clone()),
            ),
            (
                HomeSection::AiredRecently,
                SectionData::AiredRecently(value.aired_recently.clone()),
            ),
            (
                HomeSection::AnimeCarousel,
                SectionData::AnimeCarousel(value.anime_carousel.clone()),
            ),
        ] {
            let state = if home.offline {
                SectionState::Offline(data, home.updated_at)
            } else {
                SectionState::Ready(data)
            };
            self.set(section, state);
        }
    }
}

/// The status groups the library widget renders, in order. A Repeating entry
/// sits with Watching: both are shows the user is currently working through.
const STATUS_ORDER: [ListStatus; 5] = [
    ListStatus::Current,
    ListStatus::Paused,
    ListStatus::Planning,
    ListStatus::Completed,
    ListStatus::Dropped,
];

#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a clamped, non-negative fraction is stored as a whole percentage for the progress bar"
)]
fn percentage(position: f64, duration: f64) -> u8 {
    (position * 100.0 / duration).clamp(0.0, 100.0) as u8
}

/// The next playable episode of each media with local files. A saved resume
/// position wins; otherwise the first file from `progress + 1` on is used.
/// Resumed episodes sort ahead of the rest, newest position first.
fn next_episodes(
    entries: &[HomeEntry],
    files: &[LocalFile],
    resume: &[ResumePoint],
) -> Vec<ContinueEpisode> {
    let mut playable: HashMap<MediaId, Vec<&LocalFile>> = HashMap::new();
    for file in files {
        if file.kind == "episode"
            && !file.ignored
            && !file.shelved
            && file.episode.is_some()
            && let Some(media) = file.media_id
        {
            playable.entry(media).or_default().push(file);
        }
    }
    let mut episodes: Vec<(bool, SystemTime, ContinueEpisode)> = Vec::new();
    for item in entries {
        if matches!(
            item.entry.status,
            ListStatus::Completed | ListStatus::Dropped
        ) {
            continue;
        }
        let Some(candidates) = playable.get(&item.entry.media) else {
            continue;
        };
        let saved = resume.iter().find_map(|point| {
            if point.media != item.entry.media || point.position < engine::MIN_RESUME {
                return None;
            }
            let duration = point.duration?;
            if point.position >= duration {
                return None;
            }
            let file = candidates
                .iter()
                .find(|file| file.episode == Some(point.episode))?;
            let percentage = percentage(point.position, duration);
            Some((
                true,
                SystemTime::UNIX_EPOCH
                    + std::time::Duration::from_secs(u64::try_from(point.updated_at).unwrap_or(0)),
                ContinueEpisode {
                    entry: item.clone(),
                    episode: point.episode,
                    path: file.path.clone(),
                    progress: Some(percentage),
                },
            ))
        });
        let next = item.entry.progress.saturating_add(1);
        let chosen = saved.or_else(|| {
            candidates
                .iter()
                .filter(|file| file.episode.is_some_and(|episode| episode >= next))
                .min_by_key(|file| file.episode)
                .and_then(|file| {
                    file.episode.map(|episode| {
                        (
                            false,
                            item.entry.updated_at,
                            ContinueEpisode {
                                entry: item.clone(),
                                episode,
                                path: file.path.clone(),
                                progress: None,
                            },
                        )
                    })
                })
        });
        if let Some(chosen) = chosen {
            episodes.push(chosen);
        }
    }
    episodes.sort_by_key(|(resumed, updated, _)| (!*resumed, std::cmp::Reverse(*updated)));
    episodes
        .into_iter()
        .map(|(_, _, episode)| episode)
        .collect()
}

/// Groups `entries` with local files by list status, dropping empty groups.
fn status_groups(entries: &[HomeEntry], matched: &HashSet<MediaId>) -> Vec<LibraryGroup> {
    STATUS_ORDER
        .iter()
        .filter_map(|status| {
            let mut group: Vec<HomeEntry> = entries
                .iter()
                .filter(|item| {
                    let entry_status = if item.entry.status == ListStatus::Repeating {
                        ListStatus::Current
                    } else {
                        item.entry.status
                    };
                    matched.contains(&item.entry.media) && entry_status == *status
                })
                .cloned()
                .collect();
            if group.is_empty() {
                return None;
            }
            group.sort_by_key(|item| std::cmp::Reverse(item.entry.updated_at));
            Some(LibraryGroup {
                status: *status,
                entries: group,
            })
        })
        .collect()
}

/// The counts in the library's stats strip.
fn stats(files: &[LocalFile]) -> LibraryStats {
    LibraryStats {
        files: files.len(),
        bytes: files.iter().fold(0_u64, |total, file| {
            total.saturating_add(u64::try_from(file.size).unwrap_or(0))
        }),
        media: files
            .iter()
            .filter_map(|file| file.media_id)
            .collect::<HashSet<_>>()
            .len(),
        unmatched: files.iter().filter(|file| file.media_id.is_none()).count(),
    }
}

/// Applies `change` to the store and re-renders it, if it is still alive.
fn update(
    store: &WeakEntity<HomeStore>,
    cx: &mut gpui_kit::AsyncApp,
    change: impl FnOnce(&mut HomeStore),
) {
    let _ = store.update(cx, |store, cx| {
        change(store);
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use db::{LocalFile, ResumePoint};
    use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};
    use engine::{BoxFuture, EngineError, EngineServices, LibraryMediaInfo, ListedEntry};
    use gpui_kit::{AppContext, TestAppContext};
    use metadata::{
        AiringEpisode, Cached, Error as MetadataError, HomeLoad, Load, MediaSummary, MediaTitle,
        Refresh,
    };
    use tokio::runtime::{Builder, Runtime};

    use super::{HomeEntry, HomeSection, HomeStore, LibraryStats, SectionData, SectionState};
    use crate::bridge;

    /// The canned Home load: one media per section.
    fn home_load() -> HomeLoad {
        HomeLoad {
            trending: vec![summary(101)],
            aired_recently: vec![AiringEpisode {
                media: summary(201),
                episode: 3,
                airing_at: 1_700_000_000,
                time_until_airing: -60,
            }],
            anime_carousel: vec![summary(202)],
        }
    }

    /// A media summary with an AniList id and no other data.
    fn summary(id: u32) -> MediaSummary {
        MediaSummary {
            anilist_id: id,
            mal_id: None,
            kind: Some(MediaKind::Anime),
            title: MediaTitle::default(),
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

    /// A Local entry: deterministic, so tests can compare them.
    fn entry(media: i64, status: ListStatus, progress: u32) -> ListEntry {
        ListEntry {
            media: MediaId::new(media),
            status,
            score: Score::UNSCORED,
            progress,
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
            created_at: UNIX_EPOCH,
            updated_at: UNIX_EPOCH,
        }
    }

    fn watching() -> ListedEntry {
        (entry(1, ListStatus::Current, 3), Some(summary(1)))
    }

    fn planning() -> ListedEntry {
        (entry(2, ListStatus::Planning, 0), None)
    }

    fn home_entry((entry, media): ListedEntry) -> HomeEntry {
        HomeEntry { entry, media }
    }

    fn file(path: &str, media: MediaId, episode: u32, size: i64) -> LocalFile {
        LocalFile {
            path: PathBuf::from(path),
            size,
            modified: 0,
            title: None,
            group: None,
            season: None,
            episode: Some(episode),
            kind: "episode".to_owned(),
            media_id: Some(media),
            confidence: Some(0.9),
            locked: false,
            ignored: false,
            filler: false,
            shelved: false,
        }
    }

    fn resume(
        media: MediaId,
        episode: u32,
        position: f64,
        duration: f64,
        updated_at: i64,
    ) -> ResumePoint {
        ResumePoint {
            media,
            episode,
            position,
            duration: Some(duration),
            updated_at,
        }
    }

    fn info(media: MediaId, anilist_id: u32, title: &str) -> LibraryMediaInfo {
        LibraryMediaInfo::new(media, Some(anilist_id), Some(title.to_owned()))
    }

    fn airing(anilist_id: u32, episode: u32, time_until_airing: i32) -> AiringEpisode {
        AiringEpisode {
            media: summary(anilist_id),
            episode,
            airing_at: 0,
            time_until_airing,
        }
    }

    /// What a fake `home` refresh resolves to.
    #[derive(Clone)]
    enum RefreshMode {
        /// The refresh succeeds with the canned value.
        Refreshed(HomeLoad),
        /// The refresh can't reach AniList: the cached value, marked offline.
        Offline,
        /// The refresh never resolves, like a request still timing out.
        Pending,
    }

    /// A fake engine service: canned Home sections, with switchable failures.
    #[derive(Default)]
    struct FakeServices {
        /// When `home` serves a stale value, the refresh it serves.
        refresh: std::sync::Mutex<Option<RefreshMode>>,
        /// The entries `collection` serves.
        collection: Vec<ListedEntry>,
        /// Whether `collection` fails.
        collection_fails: AtomicBool,
        /// Whether `home` fails.
        home_fails: AtomicBool,
        /// How many times `home` was called.
        home_calls: AtomicUsize,
        /// How many times `collection` was called.
        collection_calls: AtomicUsize,
        /// The local files the library sections serve.
        files: Vec<LocalFile>,
        /// Whether `all_library_files` fails.
        files_fails: AtomicBool,
        /// The media with local files the library sections serve.
        media_info: Vec<LibraryMediaInfo>,
        /// Whether `library_media_info` fails.
        info_fails: AtomicBool,
        /// The saved resume positions.
        resume: Vec<ResumePoint>,
        /// Whether `continuity` fails.
        resume_fails: AtomicBool,
        /// The airing schedule.
        schedule: Vec<AiringEpisode>,
        /// Whether `airing_schedule` fails.
        schedule_fails: AtomicBool,
    }

    fn failure() -> EngineError {
        EngineError::Metadata(MetadataError::NotFound)
    }

    impl EngineServices for FakeServices {
        fn detail(
            &self,
            _anilist_id: u32,
        ) -> BoxFuture<Result<Load<metadata::MediaDetail>, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn local_media(
            &self,
            _kind: MediaKind,
            _ids: &[domain::ExternalId],
        ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn local_entry(
            &self,
            _media: MediaId,
        ) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn save_entry(&self, _entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn mint(
            &self,
            _kind: MediaKind,
            _ids: &[domain::ExternalId],
        ) -> BoxFuture<Result<MediaId, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
            self.home_calls.fetch_add(1, Ordering::SeqCst);
            let fails = self.home_fails.load(Ordering::SeqCst);
            let refresh = self
                .refresh
                .lock()
                .expect("the fake's refresh lock")
                .clone();
            Box::pin(async move {
                if fails {
                    return Err(failure());
                }
                let refresh = refresh.map(|mode| match mode {
                    RefreshMode::Refreshed(home) => Refresh::new(async move {
                        Ok(Cached {
                            value: home,
                            updated_at: SystemTime::now(),
                            offline: false,
                        })
                    }),
                    RefreshMode::Offline => Refresh::new(async {
                        Ok(Cached {
                            value: home_load(),
                            updated_at: UNIX_EPOCH,
                            offline: true,
                        })
                    }),
                    RefreshMode::Pending => Refresh::new(std::future::pending()),
                });
                Ok(Load {
                    cached: Cached {
                        value: home_load(),
                        updated_at: UNIX_EPOCH,
                        offline: false,
                    },
                    refresh,
                })
            })
        }

        fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
            self.collection_calls.fetch_add(1, Ordering::SeqCst);
            let entries = self.collection.clone();
            let fails = self.collection_fails.load(Ordering::SeqCst);
            Box::pin(async move { if fails { Err(failure()) } else { Ok(entries) } })
        }

        fn missed_sequels(
            &self,
            _kind: MediaKind,
        ) -> BoxFuture<Result<Vec<MediaSummary>, EngineError>> {
            Box::pin(async move { Ok(vec![summary(301)]) })
        }

        fn all_library_files(&self) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
            let files = self.files.clone();
            let fails = self.files_fails.load(Ordering::SeqCst);
            Box::pin(async move { if fails { Err(failure()) } else { Ok(files) } })
        }

        fn library_media_info(&self) -> BoxFuture<Result<Vec<LibraryMediaInfo>, EngineError>> {
            let info = self.media_info.clone();
            let fails = self.info_fails.load(Ordering::SeqCst);
            Box::pin(async move { if fails { Err(failure()) } else { Ok(info) } })
        }

        fn resume_points(&self) -> BoxFuture<Result<Vec<ResumePoint>, EngineError>> {
            let resume = self.resume.clone();
            let fails = self.resume_fails.load(Ordering::SeqCst);
            Box::pin(async move { if fails { Err(failure()) } else { Ok(resume) } })
        }

        fn airing_schedule(&self) -> BoxFuture<Result<Load<Vec<AiringEpisode>>, EngineError>> {
            let schedule = self.schedule.clone();
            let fails = self.schedule_fails.load(Ordering::SeqCst);
            Box::pin(async move {
                if fails {
                    return Err(failure());
                }
                Ok(Load {
                    cached: Cached {
                        value: schedule,
                        updated_at: UNIX_EPOCH,
                        offline: false,
                    },
                    refresh: None,
                })
            })
        }
    }

    fn runtime() -> Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime")
    }

    /// Lets the bridge tasks finish and the store apply them.
    async fn settle(cx: &mut TestAppContext) {
        cx.run_until_parked();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
    }

    /// Opens a `HomeStore` over the fake and settles its tasks.
    async fn open_store(
        cx: &mut TestAppContext,
        runtime: &Runtime,
        services: Arc<FakeServices>,
    ) -> gpui_kit::Entity<HomeStore> {
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let store = cx.new(|_| HomeStore::new(services));
        cx.update(|cx| store.update(cx, HomeStore::open));
        settle(cx).await;
        store
    }

    /// One section's state, through the store's public API.
    fn state_of(
        cx: &TestAppContext,
        store: &gpui_kit::Entity<HomeStore>,
        section: HomeSection,
    ) -> SectionState {
        store.read_with(cx, |store, _| store.state(section).clone())
    }

    #[gpui_kit::test]
    async fn the_store_fills_the_seven_sections_in_widget_order(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            collection: vec![watching(), planning()],
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        let expected = [
            SectionState::Ready(SectionData::ContinueWatching(vec![home_entry(watching())])),
            SectionState::Hidden,
            SectionState::Ready(SectionData::DiscoverHeader(home_load().trending)),
            SectionState::Ready(SectionData::MyLists(vec![
                home_entry(watching()),
                home_entry(planning()),
            ])),
            SectionState::Hidden,
            SectionState::Hidden,
            SectionState::Hidden,
            SectionState::Hidden,
            SectionState::Ready(SectionData::MissedSequels(vec![summary(301)])),
            SectionState::Ready(SectionData::AiredRecently(home_load().aired_recently)),
            SectionState::Ready(SectionData::AnimeCarousel(home_load().anime_carousel)),
            SectionState::Ready(SectionData::CenteredTitle("Autumn".to_owned())),
        ];
        assert_eq!(
            store.read_with(cx, |store, _| store.states().to_vec()),
            expected,
            "every section in widget order; continue-watching keeps Watching only"
        );
        assert_eq!(
            services.home_calls.load(Ordering::SeqCst),
            1,
            "one catalog load"
        );
        assert_eq!(
            services.collection_calls.load(Ordering::SeqCst),
            1,
            "one collection read for two sections"
        );
    }

    #[gpui_kit::test]
    async fn a_failing_source_leaves_the_other_sections_alone(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            collection: vec![watching()],
            ..FakeServices::default()
        });
        services.collection_fails.store(true, Ordering::SeqCst);
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        for section in [HomeSection::ContinueWatching, HomeSection::MyLists] {
            assert!(
                matches!(state_of(cx, &store, section), SectionState::Failed(_)),
                "{section:?} failed"
            );
        }
        assert_eq!(
            state_of(cx, &store, HomeSection::MissedSequels),
            SectionState::Ready(SectionData::MissedSequels(vec![summary(301)]))
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Ready(SectionData::DiscoverHeader(home_load().trending))
        );

        // Retrying the failed section reloads its source only.
        services.collection_fails.store(false, Ordering::SeqCst);
        cx.update(|cx| store.update(cx, |store, cx| store.retry(HomeSection::MyLists, cx)));
        settle(cx).await;
        assert_eq!(
            state_of(cx, &store, HomeSection::MyLists),
            SectionState::Ready(SectionData::MyLists(vec![home_entry(watching())]))
        );
        assert_eq!(
            services.home_calls.load(Ordering::SeqCst),
            1,
            "the catalog was not reloaded"
        );
    }

    #[gpui_kit::test]
    async fn a_failing_catalog_leaves_the_platform_sections_alone(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            collection: vec![watching()],
            ..FakeServices::default()
        });
        services.home_fails.store(true, Ordering::SeqCst);
        let store = open_store(cx, &runtime, services).await;

        for section in [
            HomeSection::DiscoverHeader,
            HomeSection::AiredRecently,
            HomeSection::AnimeCarousel,
        ] {
            assert!(
                matches!(state_of(cx, &store, section), SectionState::Failed(_)),
                "{section:?} failed"
            );
        }
        assert_eq!(
            state_of(cx, &store, HomeSection::ContinueWatching),
            SectionState::Ready(SectionData::ContinueWatching(vec![home_entry(watching())]))
        );
    }

    #[gpui_kit::test]
    async fn the_catalog_shows_the_cached_value_then_the_refresh(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            refresh: std::sync::Mutex::new(Some(RefreshMode::Refreshed(HomeLoad {
                trending: vec![summary(999)],
                ..home_load()
            }))),
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, services).await;

        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Ready(SectionData::DiscoverHeader(vec![summary(999)])),
            "the refreshed trending anime is on screen"
        );
    }

    #[gpui_kit::test]
    async fn an_unreachable_refresh_marks_the_cached_value_offline(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            refresh: std::sync::Mutex::new(Some(RefreshMode::Offline)),
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Offline(
                SectionData::DiscoverHeader(home_load().trending),
                UNIX_EPOCH,
            ),
            "the cached value shows under the offline state"
        );
    }

    #[gpui_kit::test]
    async fn returning_to_home_offline_keeps_the_banner_while_the_refresh_runs(
        cx: &mut TestAppContext,
    ) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            refresh: std::sync::Mutex::new(Some(RefreshMode::Offline)),
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;
        *services.refresh.lock().expect("the fake's refresh lock") = Some(RefreshMode::Pending);

        cx.update(|cx| store.update(cx, HomeStore::open));
        settle(cx).await;

        assert!(
            matches!(
                state_of(cx, &store, HomeSection::DiscoverHeader),
                SectionState::Offline(..)
            ),
            "the offline banner stays while the new refresh is pending"
        );
    }

    /// The episodes behind the library widget, through the store's API.
    fn episodes_of(
        cx: &TestAppContext,
        store: &gpui_kit::Entity<HomeStore>,
    ) -> Vec<super::ContinueEpisode> {
        match state_of(cx, store, HomeSection::AnimeContinueWatching) {
            SectionState::Ready(SectionData::AnimeContinueWatching(episodes)) => episodes,
            _ => Vec::new(),
        }
    }

    #[gpui_kit::test]
    async fn a_resumed_episode_leads_and_others_start_at_progress_plus_one(
        cx: &mut TestAppContext,
    ) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            collection: vec![
                (entry(1, ListStatus::Current, 3), Some(summary(1))),
                (entry(2, ListStatus::Current, 5), Some(summary(2))),
                (entry(3, ListStatus::Current, 0), Some(summary(3))),
                (entry(4, ListStatus::Current, 2), Some(summary(4))),
            ],
            files: vec![
                file("/a/03.mkv", MediaId::new(1), 3, 100),
                file("/a/04.mkv", MediaId::new(1), 4, 100),
                file("/b/05.mkv", MediaId::new(2), 5, 100),
                file("/b/06.mkv", MediaId::new(2), 6, 100),
                file("/c/01.mkv", MediaId::new(3), 1, 100),
                file("/d/03.mkv", MediaId::new(4), 3, 100),
            ],
            resume: vec![
                resume(MediaId::new(1), 3, 120.0, 1200.0, 10),
                resume(MediaId::new(2), 5, 600.0, 1200.0, 20),
                // At the file's end: no longer in progress, so fall back.
                resume(MediaId::new(4), 3, 1200.0, 1200.0, 30),
            ],
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        let episodes = episodes_of(cx, &store);
        let chosen: Vec<_> = episodes
            .iter()
            .map(|item| {
                (
                    item.entry.entry.media,
                    item.episode,
                    item.path.clone(),
                    item.progress,
                )
            })
            .collect();
        assert!(
            !episodes.is_empty(),
            "the continue-watching widget has data"
        );
        assert_eq!(
            chosen,
            vec![
                (MediaId::new(2), 5, PathBuf::from("/b/05.mkv"), Some(50)),
                (MediaId::new(1), 3, PathBuf::from("/a/03.mkv"), Some(10)),
                (MediaId::new(3), 1, PathBuf::from("/c/01.mkv"), None),
                (MediaId::new(4), 3, PathBuf::from("/d/03.mkv"), None),
            ],
            "the newest resume leads, then progress + 1; a finished resume is ignored"
        );
        assert_eq!(
            services.collection_calls.load(Ordering::SeqCst),
            1,
            "the library reused the one collection read"
        );
    }

    #[gpui_kit::test]
    async fn library_widgets_group_by_status_count_files_and_filter_the_schedule(
        cx: &mut TestAppContext,
    ) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            collection: vec![
                (entry(1, ListStatus::Current, 3), Some(summary(101))),
                (entry(2, ListStatus::Repeating, 3), Some(summary(102))),
                (entry(3, ListStatus::Paused, 1), Some(summary(103))),
                (entry(4, ListStatus::Planning, 0), Some(summary(104))),
                (entry(5, ListStatus::Completed, 12), Some(summary(105))),
                (entry(6, ListStatus::Dropped, 2), Some(summary(106))),
            ],
            files: vec![
                file("/a/01.mkv", MediaId::new(1), 1, 100),
                file("/a/02.mkv", MediaId::new(1), 2, 200),
                file("/b/01.mkv", MediaId::new(2), 1, 300),
                file("/c/01.mkv", MediaId::new(3), 1, 400),
                file("/d/01.mkv", MediaId::new(4), 1, 500),
                file("/e/01.mkv", MediaId::new(5), 1, 600),
                file("/f/01.mkv", MediaId::new(6), 1, 700),
                LocalFile {
                    media_id: None,
                    ..file("/loose.mkv", MediaId::new(9), 1, 900)
                },
            ],
            media_info: vec![
                info(MediaId::new(1), 101, "One"),
                info(MediaId::new(3), 103, "Three"),
            ],
            schedule: vec![
                airing(999, 1, 60),
                airing(103, 8, 7200),
                airing(101, 12, 3600),
            ],
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        let groups = match state_of(cx, &store, HomeSection::AnimeLibrary) {
            SectionState::Ready(SectionData::AnimeLibrary(groups)) => groups,
            _ => Vec::new(),
        };
        assert_eq!(
            groups.iter().map(|group| group.status).collect::<Vec<_>>(),
            vec![
                ListStatus::Current,
                ListStatus::Paused,
                ListStatus::Planning,
                ListStatus::Completed,
                ListStatus::Dropped,
            ],
            "the five statuses in order, with Repeating under Watching"
        );
        assert_eq!(
            groups.first().map(|group| group.entries.len()),
            Some(2),
            "Repeating joins Watching"
        );

        assert_eq!(
            state_of(cx, &store, HomeSection::LocalAnimeLibraryStats),
            SectionState::Ready(SectionData::LocalAnimeLibraryStats(LibraryStats {
                files: 8,
                bytes: 3700,
                media: 6,
                unmatched: 1,
            }))
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::LocalAnimeLibrary),
            SectionState::Ready(SectionData::LocalAnimeLibrary(vec![
                info(MediaId::new(1), 101, "One"),
                info(MediaId::new(3), 103, "Three"),
            ]))
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::LibraryUpcomingEpisodes),
            SectionState::Ready(SectionData::LibraryUpcomingEpisodes(vec![
                airing(101, 12, 3600),
                airing(103, 8, 7200),
            ])),
            "only library media, soonest first"
        );
    }

    #[gpui_kit::test]
    async fn an_empty_library_hides_every_library_widget(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            media_info: vec![info(MediaId::new(1), 101, "One")],
            schedule: vec![airing(101, 1, 60)],
            ..FakeServices::default()
        });
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        for section in [
            HomeSection::AnimeContinueWatching,
            HomeSection::AnimeLibrary,
            HomeSection::LocalAnimeLibrary,
            HomeSection::LocalAnimeLibraryStats,
            HomeSection::LibraryUpcomingEpisodes,
        ] {
            assert_eq!(
                state_of(cx, &store, section),
                SectionState::Hidden,
                "{section:?} is not part of an empty Home"
            );
        }
        assert_eq!(
            state_of(cx, &store, HomeSection::MyLists),
            SectionState::Ready(SectionData::MyLists(Vec::new())),
            "the non-library widgets stay"
        );
    }

    #[gpui_kit::test]
    async fn a_failed_library_call_only_fails_the_widgets_that_need_it(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        let services = Arc::new(FakeServices {
            collection: vec![(entry(1, ListStatus::Current, 3), Some(summary(1)))],
            files: vec![file("/a/03.mkv", MediaId::new(1), 3, 100)],
            media_info: vec![info(MediaId::new(1), 101, "One")],
            schedule: vec![airing(101, 4, 3600)],
            ..FakeServices::default()
        });
        services.files_fails.store(true, Ordering::SeqCst);
        let store = open_store(cx, &runtime, Arc::clone(&services)).await;

        for section in [
            HomeSection::AnimeContinueWatching,
            HomeSection::AnimeLibrary,
            HomeSection::LocalAnimeLibraryStats,
        ] {
            assert!(
                matches!(state_of(cx, &store, section), SectionState::Failed(_)),
                "{section:?} needs the file table"
            );
        }
        assert_eq!(
            state_of(cx, &store, HomeSection::LocalAnimeLibrary),
            SectionState::Ready(SectionData::LocalAnimeLibrary(vec![info(
                MediaId::new(1),
                101,
                "One"
            )])),
            "the media list does not need the file table"
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::LibraryUpcomingEpisodes),
            SectionState::Ready(SectionData::LibraryUpcomingEpisodes(vec![airing(
                101, 4, 3600
            )]))
        );

        services.files_fails.store(false, Ordering::SeqCst);
        services.schedule_fails.store(true, Ordering::SeqCst);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.retry(HomeSection::LibraryUpcomingEpisodes, cx);
            });
        });
        settle(cx).await;
        assert!(
            matches!(
                state_of(cx, &store, HomeSection::LibraryUpcomingEpisodes),
                SectionState::Failed(_)
            ),
            "the schedule widget has its own error"
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::AnimeLibrary),
            SectionState::Ready(SectionData::AnimeLibrary(vec![super::LibraryGroup {
                status: ListStatus::Current,
                entries: vec![home_entry((
                    entry(1, ListStatus::Current, 3),
                    Some(summary(1))
                ))],
            }])),
            "the other library widgets recovered"
        );
    }
}
