//! The Home screen's fixed widget set: the Home store (#61).
//!
//! The store holds every section's state, and each section resolves on its
//! own, so one failing section doesn't stop the others. The platform
//! sections come from one collection read and one missed-sequels read; the
//! catalog sections come from one stale-first Home load: the cached value
//! shows at once, and a refresh that can't reach AniList marks the sections
//! `Offline` over the cached data.

use std::{sync::Arc, time::SystemTime};

use domain::{ListEntry, ListStatus, MediaKind};
use engine::EngineServices;
use gpui_kit::{Context, Task, WeakEntity};
use metadata::{AiringEpisode, Cached, HomeLoad, MediaSummary};

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
    /// The discover header: the catalog's trending anime.
    DiscoverHeader,
    /// The user's list.
    MyLists,
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
    pub const ALL: [Self; 7] = [
        Self::ContinueWatching,
        Self::DiscoverHeader,
        Self::MyLists,
        Self::MissedSequels,
        Self::AiredRecently,
        Self::AnimeCarousel,
        Self::CenteredTitle,
    ];

    /// The index of the section in [`Self::ALL`].
    fn index(self) -> usize {
        match self {
            Self::ContinueWatching => 0,
            Self::DiscoverHeader => 1,
            Self::MyLists => 2,
            Self::MissedSequels => 3,
            Self::AiredRecently => 4,
            Self::AnimeCarousel => 5,
            Self::CenteredTitle => 6,
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

/// One Home widget's data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "a section renders exactly one data variant"
)]
pub enum SectionData {
    /// The continue-watching header: Watching entries, most recent first.
    ContinueWatching(Vec<HomeEntry>),
    /// The discover header: trending anime.
    DiscoverHeader(Vec<MediaSummary>),
    /// The user's list, most recently updated first.
    MyLists(Vec<HomeEntry>),
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
        }
        cx.notify();
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
            let (watching, lists) = match collection {
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
                        SectionState::Ready(SectionData::MyLists(entries)),
                    )
                }
                Err(message) => (
                    SectionState::Failed(message.clone()),
                    SectionState::Failed(message),
                ),
            };
            update(&store, cx, |store| {
                store.set(HomeSection::ContinueWatching, watching);
                store.set(HomeSection::MyLists, lists);
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
        sync::{
            Arc,
            atomic::{AtomicBool, AtomicUsize, Ordering},
        },
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};
    use engine::{BoxFuture, EngineError, EngineServices, ListedEntry};
    use gpui_kit::{AppContext, TestAppContext};
    use metadata::{
        AiringEpisode, Cached, Error as MetadataError, HomeLoad, Load, MediaSummary, MediaTitle,
        Refresh,
    };
    use tokio::runtime::{Builder, Runtime};

    use super::{HomeEntry, HomeSection, HomeStore, SectionData, SectionState};
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
            SectionData::ContinueWatching(vec![home_entry(watching())]),
            SectionData::DiscoverHeader(home_load().trending),
            SectionData::MyLists(vec![home_entry(watching()), home_entry(planning())]),
            SectionData::MissedSequels(vec![summary(301)]),
            SectionData::AiredRecently(home_load().aired_recently),
            SectionData::AnimeCarousel(home_load().anime_carousel),
            SectionData::CenteredTitle("Autumn".to_owned()),
        ]
        .map(SectionState::Ready);
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
}
