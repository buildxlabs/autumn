//! The Home screen's fixed widget set: the Home store (#61).
//!
//! The store holds every section's state; each section loads on its own, so
//! one failing section doesn't stop the others. The catalog sections come
//! from the stale-first cache: the cached value is ready at once and the
//! refresh, when it fails to reach AniList, marks the section `Offline`
//! over the cached data.

use std::{sync::Arc, time::SystemTime};

use domain::{ListEntry, MediaKind};
use engine::EngineServices;
use gpui_kit::{Context, Task, WeakEntity};
use metadata::{AiringEpisode, HomeLoad, Load, MediaSummary};

use crate::entry_store::call;

/// The Home screen's fixed widget set, in order (#23, the Phase 1 exit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Home screen matches every section, so a new section must not compile until it has one"
)]
pub enum HomeSection {
    /// The continue-watching header: the platform's entries with progress.
    ContinueWatching,
    /// The discover header: the catalog's trending anime.
    DiscoverHeader,
    /// The user's lists.
    MyLists,
    /// The missed sequels.
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
    /// The request failed; the shell shows the section's error.
    Failed(String),
    /// Cached data while the refresh is offline; the shell shows the
    /// offline banner over it.
    Offline(SectionData, SystemTime),
}

/// One Home widget's data.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "a section renders exactly one data variant"
)]
pub enum SectionData {
    /// The continue-watching header: entries with progress.
    ContinueWatching(Vec<ListEntry>),
    /// The discover header: trending anime.
    DiscoverHeader(Vec<MediaSummary>),
    /// The user's lists.
    MyLists(Vec<ListEntry>),
    /// The missed sequels.
    MissedSequels(Vec<ListEntry>),
    /// The aired-recently carousel.
    AiredRecently(Vec<AiringEpisode>),
    /// The anime carousel.
    AnimeCarousel(Vec<MediaSummary>),
    /// The centered title.
    CenteredTitle(String),
}

/// The app name shown in the centered-title widget.
const TITLE: &str = "Autumn";

/// The Home store: the fixed Home widget set, each section's state, and the
/// data behind it.
pub struct HomeStore {
    services: Arc<dyn EngineServices>,
    /// Each section's state, in [`HomeSection::ALL`] order.
    states: Vec<SectionState>,
    /// The in-flight open task.
    task: Option<Task<()>>,
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
        Self {
            services,
            states: HomeSection::ALL.map(|_| SectionState::Loading).to_vec(),
            task: None,
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

    /// Starts the Home sections: the catalog sections (stale-first) and the
    /// active platform's sections. Each section sets its own state when it
    /// resolves; one failing doesn't stop the others.
    pub fn open(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.task = Some(cx.spawn(async move |store, cx| {
            for section in HomeSection::ALL {
                match section {
                    HomeSection::ContinueWatching => {
                        let result = call(cx, services.collection(MediaKind::Anime)).await;
                        set_state(
                            &store,
                            cx,
                            section,
                            match result {
                                Ok(entries) => SectionState::Ready(SectionData::ContinueWatching(
                                    entries
                                        .into_iter()
                                        .filter(|entry| entry.progress > 0)
                                        .collect(),
                                )),
                                Err(message) => SectionState::Failed(message),
                            },
                        );
                    }
                    HomeSection::MyLists => {
                        let result = call(cx, services.collection(MediaKind::Anime)).await;
                        set_state(
                            &store,
                            cx,
                            section,
                            match result {
                                Ok(entries) => SectionState::Ready(SectionData::MyLists(entries)),
                                Err(message) => SectionState::Failed(message),
                            },
                        );
                    }
                    HomeSection::MissedSequels => {
                        let result = call(cx, services.missed_sequels(MediaKind::Anime)).await;
                        set_state(
                            &store,
                            cx,
                            section,
                            match result {
                                Ok(entries) => {
                                    SectionState::Ready(SectionData::MissedSequels(entries))
                                }
                                Err(message) => SectionState::Failed(message),
                            },
                        );
                    }
                    HomeSection::DiscoverHeader
                    | HomeSection::AiredRecently
                    | HomeSection::AnimeCarousel => {
                        let result = call(cx, services.home()).await;
                        catalog_section(
                            &store,
                            cx,
                            section,
                            result,
                            |home: &HomeLoad| match section {
                                HomeSection::DiscoverHeader => {
                                    SectionData::DiscoverHeader(home.trending.clone())
                                }
                                HomeSection::AiredRecently => {
                                    SectionData::AiredRecently(home.aired_recently.clone())
                                }
                                HomeSection::AnimeCarousel => {
                                    SectionData::AnimeCarousel(home.anime_carousel.clone())
                                }
                                _ => SectionData::CenteredTitle(String::new()),
                            },
                        )
                        .await;
                    }
                    HomeSection::CenteredTitle => set_state(
                        &store,
                        cx,
                        section,
                        SectionState::Ready(SectionData::CenteredTitle(TITLE.to_owned())),
                    ),
                }
            }
        }));
        cx.notify();
    }
}

/// Sets one section's state on the store.
fn set_state(
    store: &WeakEntity<HomeStore>,
    cx: &mut gpui_kit::AsyncApp,
    section: HomeSection,
    state: SectionState,
) {
    let _ = store.update(cx, |store, cx| {
        if let Some(slot) = store.states.get_mut(section.index()) {
            *slot = state;
        }
        cx.notify();
    });
}

/// A catalog section: the stale-first load's cached value now, then the
/// refresh when it resolves, or offline when it can't reach AniList.
///
/// `data_of` picks the section's data out of the Home load, for both the
/// cached value and the refreshed one.
async fn catalog_section(
    store: &WeakEntity<HomeStore>,
    cx: &mut gpui_kit::AsyncApp,
    section: HomeSection,
    result: Result<Load<HomeLoad>, String>,
    data_of: impl Fn(&HomeLoad) -> SectionData,
) {
    let load = match result {
        Ok(load) => load,
        Err(message) => {
            set_state(store, cx, section, SectionState::Failed(message));
            return;
        }
    };
    let updated_at = load.cached.updated_at;
    let data = data_of(&load.cached.value);
    set_state(store, cx, section, SectionState::Ready(data.clone()));
    let Some(refresh) = load.refresh else {
        return;
    };
    match refresh.await {
        Ok(fresh) => {
            set_state(
                store,
                cx,
                section,
                SectionState::Ready(data_of(&fresh.value)),
            );
        }
        Err(_) => set_state(store, cx, section, SectionState::Offline(data, updated_at)),
    }
}

#[cfg(test)]
mod tests {
    use std::{
        sync::Arc,
        time::{Duration, SystemTime, UNIX_EPOCH},
    };

    use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};
    use engine::{BoxFuture, EngineError, EngineServices};
    use gpui_kit::{AppContext, TestAppContext};
    use metadata::{
        AiringEpisode, Cached, Error as MetadataError, HomeLoad, Load, MediaSummary, MediaTitle,
        Refresh,
    };
    use tokio::runtime::{Builder, Runtime};

    use super::{HomeSection, HomeStore, SectionData, SectionState};
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
    fn entry(media: i64, progress: u32) -> ListEntry {
        ListEntry {
            media: MediaId::new(media),
            status: ListStatus::Current,
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

    /// What a fake `home` refresh resolves to.
    #[derive(Clone)]
    enum RefreshMode {
        /// The refresh succeeds with the canned value.
        Refreshed(HomeLoad),
        /// The refresh fails, so the cached value is offline.
        Offline,
    }

    /// A fake engine service: canned Home sections.
    struct FakeServices {
        /// The load `home` serves, without a refresh.
        home: HomeLoad,
        /// When `home` serves a stale value, the refresh it serves.
        refresh: Option<RefreshMode>,
        /// The "last updated" time of the canned load.
        updated_at: SystemTime,
        /// The entries `collection` serves.
        collection: Vec<ListEntry>,
        /// Whether `collection` fails.
        collection_fails: bool,
        /// The entries `missed_sequels` serves.
        missed: Vec<ListEntry>,
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
            let home = self.home.clone();
            let updated_at = self.updated_at;
            let refresh = self.refresh.clone();
            Box::pin(async move {
                let refresh = refresh.map(|mode| match mode {
                    RefreshMode::Refreshed(home) => Refresh::new(async move {
                        Ok(Cached {
                            value: home,
                            updated_at: SystemTime::now(),
                            offline: false,
                        })
                    }),
                    RefreshMode::Offline => Refresh::new(async { Err(MetadataError::NotFound) }),
                });
                Ok(Load {
                    cached: Cached {
                        value: home,
                        updated_at,
                        offline: false,
                    },
                    refresh,
                })
            })
        }

        fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
            let entries = self.collection.clone();
            let fails = self.collection_fails;
            Box::pin(async move {
                if fails {
                    Err(EngineError::Metadata(MetadataError::NotFound))
                } else {
                    Ok(entries)
                }
            })
        }

        fn missed_sequels(
            &self,
            _kind: MediaKind,
        ) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
            let entries = self.missed.clone();
            Box::pin(async move { Ok(entries) })
        }
    }

    fn runtime() -> Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime")
    }

    /// Opens a `HomeStore` over the fake and settles its tasks.
    async fn open_store(
        cx: &mut TestAppContext,
        services: Arc<dyn EngineServices>,
    ) -> gpui_kit::Entity<HomeStore> {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let store = cx.new(|_| HomeStore::new(services));
        cx.update(|cx| store.update(cx, HomeStore::open));
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
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
    async fn the_store_exposes_the_seven_sections_in_widget_order(cx: &mut TestAppContext) {
        let services: Arc<dyn EngineServices> = Arc::new(FakeServices {
            home: home_load(),
            refresh: None,
            updated_at: UNIX_EPOCH,
            collection: vec![entry(1, 3), entry(2, 0)],
            collection_fails: false,
            missed: vec![entry(3, 0)],
        });
        let store = open_store(cx, services).await;

        assert_eq!(store.read_with(cx, |store, _| store.states().len()), 7);
        assert_eq!(
            state_of(cx, &store, HomeSection::ContinueWatching),
            SectionState::Ready(SectionData::ContinueWatching(vec![entry(1, 3)])),
            "entries with progress"
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Ready(SectionData::DiscoverHeader(home_load().trending)),
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::MyLists),
            SectionState::Ready(SectionData::MyLists(vec![entry(1, 3), entry(2, 0)])),
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::MissedSequels),
            SectionState::Ready(SectionData::MissedSequels(vec![entry(3, 0)])),
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::AiredRecently),
            SectionState::Ready(SectionData::AiredRecently(home_load().aired_recently)),
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::AnimeCarousel),
            SectionState::Ready(SectionData::AnimeCarousel(home_load().anime_carousel)),
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::CenteredTitle),
            SectionState::Ready(SectionData::CenteredTitle("Autumn".to_owned())),
        );
    }

    #[gpui_kit::test]
    async fn a_failing_section_does_not_affect_the_others(cx: &mut TestAppContext) {
        let services: Arc<dyn EngineServices> = Arc::new(FakeServices {
            home: home_load(),
            refresh: None,
            updated_at: UNIX_EPOCH,
            collection: Vec::new(),
            collection_fails: true,
            missed: vec![entry(3, 0)],
        });
        let store = open_store(cx, services).await;

        assert!(matches!(
            state_of(cx, &store, HomeSection::ContinueWatching),
            SectionState::Failed(_)
        ));
        assert!(matches!(
            state_of(cx, &store, HomeSection::MyLists),
            SectionState::Failed(_)
        ));
        assert_eq!(
            state_of(cx, &store, HomeSection::MissedSequels),
            SectionState::Ready(SectionData::MissedSequels(vec![entry(3, 0)]))
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Ready(SectionData::DiscoverHeader(home_load().trending))
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::AiredRecently),
            SectionState::Ready(SectionData::AiredRecently(home_load().aired_recently))
        );
        assert_eq!(
            state_of(cx, &store, HomeSection::AnimeCarousel),
            SectionState::Ready(SectionData::AnimeCarousel(home_load().anime_carousel))
        );
    }

    #[gpui_kit::test]
    async fn the_catalog_shows_the_cached_value_then_the_refresh(cx: &mut TestAppContext) {
        let services: Arc<dyn EngineServices> = Arc::new(FakeServices {
            home: home_load(),
            refresh: Some(RefreshMode::Refreshed(HomeLoad {
                trending: vec![summary(999)],
                ..home_load()
            })),
            updated_at: UNIX_EPOCH,
            collection: Vec::new(),
            collection_fails: false,
            missed: Vec::new(),
        });
        let store = open_store(cx, services).await;

        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Ready(SectionData::DiscoverHeader(vec![summary(999)])),
            "the refreshed trending anime is on screen"
        );
    }

    #[gpui_kit::test]
    async fn an_offline_refresh_shows_the_cached_value(cx: &mut TestAppContext) {
        let services = Arc::new(FakeServices {
            home: home_load(),
            refresh: Some(RefreshMode::Offline),
            updated_at: UNIX_EPOCH,
            collection: Vec::new(),
            collection_fails: false,
            missed: Vec::new(),
        });
        let store = open_store(cx, services).await;

        assert_eq!(
            state_of(cx, &store, HomeSection::DiscoverHeader),
            SectionState::Offline(
                SectionData::DiscoverHeader(home_load().trending),
                UNIX_EPOCH,
            ),
            "the cached value shows under the offline state"
        );
    }
}
