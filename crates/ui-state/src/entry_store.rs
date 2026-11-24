//! The Anime Entry screen's store (#58): the Entry detail, the Local list
//! entry, and the list editor draft.

use std::{sync::Arc, time::SystemTime};

use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};
use engine::{BoxFuture, EngineError, EngineServices};
use gpui_kit::{Context, Task, WeakEntity};
use metadata::MediaDetail;

use crate::bridge;

/// The Entry screen's state.
#[derive(Debug, Clone)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Entry screen matches every state"
)]
#[expect(
    clippy::large_enum_variant,
    reason = "the Entry detail lives only in the one Ready state; the others are small"
)]
pub enum EntryState {
    /// The Entry detail is loading.
    Loading,
    /// The Entry is open: the detail, the Local entry, and the editor draft.
    Ready {
        /// The Entry's metadata: header, synopsis, characters, relations and recommendations.
        detail: MediaDetail,
        /// The Local entry, when the media is on the user's list.
        entry: Option<ListEntry>,
        /// The editor draft: status, progress, score, dates and notes.
        draft: EntryDraft,
    },
    /// The fetch failed.
    Failed(String),
}

/// The Local list editor's draft: the values a save writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryDraft {
    /// Where the media sits in the user's list.
    pub status: ListStatus,
    /// Episodes watched.
    pub progress: u32,
    /// The overall score, raw 0–100.
    pub score: Score,
    /// When the user started.
    pub started_at: FuzzyDate,
    /// When the user finished.
    pub completed_at: FuzzyDate,
    /// Free-text notes.
    pub notes: String,
}

impl Default for EntryDraft {
    fn default() -> Self {
        Self {
            status: ListStatus::Planning,
            progress: 0,
            score: Score::UNSCORED,
            started_at: FuzzyDate::UNKNOWN,
            completed_at: FuzzyDate::UNKNOWN,
            notes: String::new(),
        }
    }
}

impl EntryDraft {
    /// The draft of `entry`, for a media already on the user's list.
    fn from_entry(entry: &ListEntry) -> Self {
        Self {
            status: entry.status,
            progress: entry.progress,
            score: entry.score,
            started_at: entry.started_at,
            completed_at: entry.completed_at,
            notes: entry.notes.clone(),
        }
    }
}

/// The Anime Entry screen's store.
///
/// Opens the Entry detail and the Local entry, and writes the editor draft
/// through the active platform (Local). A task is stored for each fetch, so
/// a newer `open` or `save` cancels an older one.
pub struct EntryStore {
    services: Arc<dyn EngineServices>,
    state: EntryState,
    /// The media identity: found on open, minted on the first save.
    media: Option<MediaId>,
    open_task: Option<Task<()>>,
    /// The stale-detail refresh; replacing it cancels the older request.
    refresh_task: Option<Task<()>>,
    save_task: Option<Task<()>>,
    /// A save failure, shown inline so the detail stays on screen.
    save_error: Option<String>,
}

impl std::fmt::Debug for EntryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntryStore")
            .field("state", &self.state)
            .field("media", &self.media)
            .finish_non_exhaustive()
    }
}

impl EntryStore {
    /// A store with no Entry open yet.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services,
            state: EntryState::Loading,
            media: None,
            open_task: None,
            refresh_task: None,
            save_task: None,
            save_error: None,
        }
    }

    /// The Entry screen's state.
    #[must_use]
    pub fn state(&self) -> &EntryState {
        &self.state
    }

    /// The last save failure, to show without replacing the detail.
    #[must_use]
    pub fn save_error(&self) -> Option<&str> {
        self.save_error.as_deref()
    }

    /// The AniList id of the Entry currently shown, if one is open.
    fn current_id(&self) -> Option<u32> {
        match &self.state {
            EntryState::Ready { detail, .. } => Some(detail.summary.anilist_id),
            _ => None,
        }
    }

    /// The editor draft, while the Entry is open.
    #[must_use]
    pub fn draft(&self) -> Option<&EntryDraft> {
        match &self.state {
            EntryState::Ready { draft, .. } => Some(draft),
            _ => None,
        }
    }

    /// The editor draft, while the Entry is open.
    #[must_use]
    pub fn draft_mut(&mut self) -> Option<&mut EntryDraft> {
        match &mut self.state {
            EntryState::Ready { draft, .. } => Some(draft),
            _ => None,
        }
    }

    /// Opens the Entry of AniList `anilist_id`: fetches the detail and the
    /// Local entry, and resets the draft to what the list holds.
    pub fn open(&mut self, anilist_id: u32, cx: &mut Context<'_, Self>) {
        // A different Entry must not show the previous one's detail while it
        // loads. Replacing the task already cancels the older fetch.
        self.state = EntryState::Loading;
        self.media = None;
        self.save_error = None;
        // A refresh for the previous Entry must not write into this one.
        self.refresh_task = None;
        let services = Arc::clone(&self.services);
        self.open_task = Some(cx.spawn(async move |store, cx| {
            let load = match call(cx, services.detail(anilist_id)).await {
                Ok(load) => load,
                Err(message) => {
                    fail(&store, cx, message);
                    return;
                }
            };
            let metadata::Load { cached, refresh } = load;
            let detail = cached.value;
            let kind = detail.summary.kind.unwrap_or(MediaKind::Anime);
            let ids = detail.summary.external_ids();
            // The media identity, without minting: a media not yet in the
            // user's list has none until the first save.
            let media = match call(cx, services.local_media(kind, &ids)).await {
                Ok(media) => media,
                Err(message) => {
                    fail(&store, cx, message);
                    return;
                }
            };
            let entry = match media {
                None => None,
                Some(media) => match call(cx, services.local_entry(media)).await {
                    Ok(entry) => entry,
                    Err(message) => {
                        fail(&store, cx, message);
                        return;
                    }
                },
            };
            let draft = entry
                .as_ref()
                .map_or_else(EntryDraft::default, EntryDraft::from_entry);
            let _ = store.update(cx, move |store, cx| {
                store.media = media;
                store.state = EntryState::Ready {
                    detail,
                    entry,
                    draft,
                };
                // A stale detail comes with a refresh that must be awaited;
                // dropping it would cancel it. Apply its value only while
                // this Entry is still the one on screen.
                if let Some(refresh) = refresh {
                    store.refresh_task = Some(cx.spawn(async move |store, cx| {
                        let refreshed = cx.update(|cx| bridge::run(cx, refresh)).await;
                        if let Ok(Ok(cached)) = refreshed {
                            let _ = store.update(cx, |store, cx| {
                                if store.current_id() == Some(anilist_id) {
                                    if let EntryState::Ready { detail, .. } = &mut store.state {
                                        *detail = cached.value;
                                    }
                                    cx.notify();
                                }
                            });
                        }
                    }));
                }
                cx.notify();
            });
        }));
        cx.notify();
    }

    /// Saves the editor draft on the active platform (Local). The first
    /// save mints the media identity on the first user fact; later saves
    /// replace the entry as stored.
    pub fn save(&mut self, cx: &mut Context<'_, Self>) {
        let EntryState::Ready {
            detail,
            entry,
            draft,
        } = &self.state
        else {
            return;
        };
        let anilist_id = detail.summary.anilist_id;
        let kind = detail.summary.kind.unwrap_or(MediaKind::Anime);
        let ids = detail.summary.external_ids();
        let (entry, draft) = (entry.clone(), draft.clone());
        let services = Arc::clone(&self.services);
        let media = self.media;
        self.save_error = None;
        self.save_task = Some(cx.spawn(async move |store, cx| {
            // The first user fact mints the media identity.
            let media = match media {
                Some(media) => media,
                None => match call(cx, services.mint(kind, &ids)).await {
                    Ok(media) => media,
                    Err(message) => {
                        save_failed(&store, cx, anilist_id, message);
                        return;
                    }
                },
            };
            // The draft as a list entry, keeping the fields the editor
            // doesn't show.
            let mut list = entry.unwrap_or_else(|| blank_entry(media));
            list.media = media;
            list.status = draft.status;
            list.progress = draft.progress;
            list.score = draft.score;
            list.started_at = draft.started_at;
            list.completed_at = draft.completed_at;
            list.notes = draft.notes.clone();
            match call(cx, services.save_entry(&list)).await {
                Ok(saved) => {
                    let _ = store.update(cx, |store, cx| {
                        // The user may have opened another Entry while the
                        // save was in flight. Never write this one's detail
                        // over the new route.
                        if store.current_id() != Some(anilist_id) {
                            return;
                        }
                        store.media = Some(media);
                        store.save_error = None;
                        // Only the list entry changes: a detail refresh
                        // or further edits may have landed during the save.
                        if let EntryState::Ready { entry, .. } = &mut store.state {
                            *entry = Some(saved);
                        }
                        cx.notify();
                    });
                }
                Err(message) => save_failed(&store, cx, anilist_id, message),
            }
        }));
        cx.notify();
    }
}

/// A [`ListEntry`] for a media with no entry yet: the Local platform's save
/// defaults, since the editor overwrites the fields it shows.
fn blank_entry(media: MediaId) -> ListEntry {
    ListEntry {
        media,
        status: ListStatus::Planning,
        score: Score::UNSCORED,
        progress: 0,
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
        created_at: SystemTime::now(),
        updated_at: SystemTime::now(),
    }
}

/// Runs one engine service call through the bridge and awaits it.
///
/// Both the bridge task and the service's own result are `Result`s; the
/// service's wins when the bridge task succeeded. Failures return the
/// message to show.
async fn call<T>(
    cx: &mut gpui_kit::AsyncApp,
    future: BoxFuture<Result<T, EngineError>>,
) -> Result<T, String>
where
    T: Send + 'static,
{
    match cx.update(|cx| bridge::run(cx, future)).await {
        Ok(Ok(value)) => Ok(value),
        Ok(Err(error)) => Err(error.to_string()),
        Err(error) => Err(error.to_string()),
    }
}

/// Sets the Entry screen's failure state.
fn fail(store: &WeakEntity<EntryStore>, cx: &mut gpui_kit::AsyncApp, message: String) {
    let _ = store.update(cx, |store, cx| {
        store.state = EntryState::Failed(message);
        cx.notify();
    });
}

/// Records a save failure inline, keeping the detail on screen. A save for
/// an Entry the user has since left has nothing to attach the error to.
fn save_failed(
    store: &WeakEntity<EntryStore>,
    cx: &mut gpui_kit::AsyncApp,
    anilist_id: u32,
    message: String,
) {
    let _ = store.update(cx, |store, cx| {
        if store.current_id() == Some(anilist_id) {
            store.save_error = Some(message);
            cx.notify();
        }
    });
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::clone_on_ref_ptr,
        reason = "the fake stays concrete for the test's field reads; EntryStore::new takes the trait object"
    )]
    #![expect(
        clippy::indexing_slicing,
        reason = "the EntryStore tests index the recorded saves and reads"
    )]

    use std::{
        sync::{Arc, Mutex},
        time::{Duration, SystemTime},
    };

    use crate::bridge;
    use domain::{ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};
    use engine::{BoxFuture, EngineError, EngineServices};
    use gpui_kit::{AppContext, TestAppContext};
    use metadata::{Cached, Load, MediaDetail, MediaSummary, MediaTitle, Refresh};
    use tokio::runtime::{Builder, Runtime};

    use super::{EntryState, EntryStore};

    const ANILIST_ID: u32 = 35_689;
    const OTHER_ID: u32 = 42_001;
    const MINTED_MEDIA: MediaId = MediaId::new(42);

    /// A fake engine service: canned Entry detail, and records of the Local
    /// state it serves and the saves and mints it receives.
    #[derive(Default)]
    struct FakeServices {
        /// The media identity `local_media` reports, if any.
        media: Arc<Mutex<Option<MediaId>>>,
        /// The Local entry `local_entry` reports for `media`, if any.
        entry: Arc<Mutex<Option<ListEntry>>>,
        /// Every entry `save_entry` was called with, oldest first.
        saved: Arc<Mutex<Vec<ListEntry>>>,
        /// Every id set `mint` was called with.
        minted: Arc<Mutex<MintedIds>>,
        /// When set, `detail` serves a stale value with this fresh title.
        refreshed_title: Arc<Mutex<Option<String>>>,
        /// How long a save takes, so a newer open can race it.
        save_delay: Duration,
        /// How long a stale detail takes to refresh.
        refresh_delay: Duration,
    }

    /// One `mint` call the fake recorded: `(kind, ids)`.
    type MintedIds = Vec<(MediaKind, Vec<ExternalId>)>;

    impl EngineServices for FakeServices {
        fn detail(&self, anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
            let refreshed = self
                .refreshed_title
                .lock()
                .expect("the fake's refresh lock")
                .clone();
            let refresh_delay = self.refresh_delay;
            Box::pin(async move {
                let mut value = detail_for(anilist_id);
                let refresh = refreshed.map(|title| {
                    Refresh::new(async move {
                        tokio::time::sleep(refresh_delay).await;
                        let mut fresh = detail_for(anilist_id);
                        fresh.description = Some(title);
                        Ok(Cached {
                            value: fresh,
                            updated_at: SystemTime::now(),
                            offline: false,
                        })
                    })
                });
                if refresh.is_some() {
                    value.description = Some("stale".to_owned());
                }
                Ok(Load {
                    cached: Cached {
                        value,
                        updated_at: SystemTime::now(),
                        offline: false,
                    },
                    refresh,
                })
            })
        }

        fn local_media(
            &self,
            _kind: MediaKind,
            _ids: &[ExternalId],
        ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
            let media = *self.media.lock().expect("the fake's media lock");
            Box::pin(async move { Ok(media) })
        }

        fn local_entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
            let entry = self.entry.lock().expect("the fake's entry lock").clone();
            Box::pin(async move { Ok(entry.filter(|entry| entry.media == media)) })
        }

        fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
            let entry = entry.clone();
            let saved = Arc::clone(&self.saved);
            let media = Arc::clone(&self.media);
            let local = Arc::clone(&self.entry);
            let delay = self.save_delay;
            Box::pin(async move {
                tokio::time::sleep(delay).await;
                saved
                    .lock()
                    .expect("the fake's saved lock")
                    .push(entry.clone());
                *media.lock().expect("the fake's media lock") = Some(entry.media);
                *local.lock().expect("the fake's entry lock") = Some(entry.clone());
                Ok(entry)
            })
        }

        fn mint(
            &self,
            kind: MediaKind,
            ids: &[ExternalId],
        ) -> BoxFuture<Result<MediaId, EngineError>> {
            let ids = ids.to_vec();
            let minted = Arc::clone(&self.minted);
            Box::pin(async move {
                minted
                    .lock()
                    .expect("the fake's minted lock")
                    .push((kind, ids));
                Ok(MINTED_MEDIA)
            })
        }

        fn home(&self) -> BoxFuture<Result<Load<metadata::HomeLoad>, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
            Box::pin(std::future::pending())
        }

        fn missed_sequels(
            &self,
            _kind: MediaKind,
        ) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
            Box::pin(std::future::pending())
        }
    }

    /// The Entry page's detail, as the engine serves it.
    fn detail_for(anilist_id: u32) -> MediaDetail {
        MediaDetail {
            summary: MediaSummary {
                anilist_id,
                mal_id: Some(27_613),
                kind: Some(MediaKind::Anime),
                title: MediaTitle {
                    romaji: Some(format!("Media {anilist_id}")),
                    ..MediaTitle::default()
                },
                poster: None,
                banner: None,
                format: None,
                status: None,
                season: None,
                season_year: None,
                episodes: Some(28),
                chapters: None,
                mean_score: Some(91),
                genres: Vec::new(),
                is_adult: false,
            },
            description: Some("<p>A sushi chef…</p>".to_owned()),
            start_date: FuzzyDate::UNKNOWN,
            end_date: FuzzyDate::UNKNOWN,
            duration: None,
            studios: Vec::new(),
            popularity: Some(12_345),
            characters: Vec::new(),
            relations: Vec::new(),
            recommendations: Vec::new(),
        }
    }

    /// A Local entry for the test's media.
    fn local_entry() -> ListEntry {
        ListEntry {
            media: MINTED_MEDIA,
            status: ListStatus::Current,
            score: Score::new(85).expect("85 is a valid score"),
            progress: 3,
            ..super::blank_entry(MINTED_MEDIA)
        }
    }

    fn runtime() -> Runtime {
        Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime")
    }

    fn open_store(cx: &mut TestAppContext, services: Arc<FakeServices>) -> EntityRef {
        let store = cx.new(|_| EntryStore::new(services));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        cx.run_until_parked();
        EntityRef { store }
    }

    struct EntityRef {
        store: gpui_kit::Entity<EntryStore>,
    }

    impl EntityRef {
        fn ready(
            &self,
            cx: &TestAppContext,
        ) -> (MediaDetail, Option<ListEntry>, super::EntryDraft) {
            match self.store.read_with(cx, |store, _| store.state().clone()) {
                EntryState::Ready {
                    detail,
                    entry,
                    draft,
                } => (detail, entry, draft),
                #[expect(
                    clippy::panic,
                    reason = "the assertion prints the Entry's actual state"
                )]
                state => panic!("the Entry should be ready: {state:?}"),
            }
        }
    }

    #[gpui_kit::test]
    async fn opening_the_entry_loads_the_detail_and_the_local_entry(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = Arc::new(FakeServices {
            media: Arc::new(Mutex::new(Some(MINTED_MEDIA))),
            entry: Arc::new(Mutex::new(Some(local_entry()))),
            ..FakeServices::default()
        });
        let opened = open_store(cx, services);
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        let (detail, entry, draft) = opened.ready(cx);

        assert_eq!(detail.summary.anilist_id, ANILIST_ID);
        assert_eq!(
            entry.as_ref().map(|entry| entry.status),
            Some(ListStatus::Current)
        );
        assert_eq!(entry.as_ref().map(|entry| entry.progress), Some(3));
        assert_eq!(
            draft.status,
            ListStatus::Current,
            "the draft starts from the Local entry"
        );
    }

    #[gpui_kit::test]
    async fn saving_the_draft_writes_it_through_the_platform_and_mints_on_the_first_fact(
        cx: &mut TestAppContext,
    ) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = Arc::new(FakeServices::default());
        let store = cx.new(|_| EntryStore::new(services.clone()));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        cx.run_until_parked();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        // Edit the draft: Watching, progress 3, a score and a start date.
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.draft_mut().expect("the Entry is ready").status = ListStatus::Current;
                store.draft_mut().expect("the Entry is ready").progress = 3;
                store.draft_mut().expect("the Entry is ready").score =
                    Score::new(85).expect("85 is a valid score");
                store.draft_mut().expect("the Entry is ready").started_at =
                    FuzzyDate::new(Some(2026), Some(11), None).expect("a valid date");
            });
        });
        cx.update(|cx| store.update(cx, EntryStore::save));
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        let saved = services.saved.lock().expect("the fake's saved lock");
        assert_eq!(saved.len(), 1, "the draft was saved once");
        assert_eq!(saved[0].status, ListStatus::Current);
        assert_eq!(saved[0].progress, 3);
        assert_eq!(saved[0].score.raw(), 85);
        assert_eq!(
            saved[0].started_at,
            FuzzyDate::new(Some(2026), Some(11), None).expect("a valid date")
        );
        assert_eq!(
            saved[0].media, MINTED_MEDIA,
            "the first save mints the media identity"
        );
        assert_eq!(
            services
                .minted
                .lock()
                .expect("the fake's minted lock")
                .len(),
            1
        );
    }

    #[gpui_kit::test]
    async fn reopening_the_entry_shows_the_saved_entry(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = Arc::new(FakeServices::default());
        let store = cx.new(|_| EntryStore::new(services.clone()));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        cx.run_until_parked();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
        // Set Watching, 3 episodes.
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.draft_mut().expect("the Entry is ready").status = ListStatus::Current;
                store.draft_mut().expect("the Entry is ready").progress = 3;
            });
        });
        store.update(cx, EntryStore::save);
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
        // The fake's Local platform now knows the media, so a reopen sees it
        // without the test writing the fake's state by hand.
        assert!(
            services
                .media
                .lock()
                .expect("the fake's media lock")
                .is_some()
        );

        // The app restarts: a new store over the same Local platform.
        let reopened = cx.new(|_| EntryStore::new(services.clone()));
        cx.update(|cx| reopened.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        match reopened.read_with(cx, |store, _| store.state().clone()) {
            EntryState::Ready { entry, .. } => {
                assert_eq!(
                    entry.as_ref().map(|entry| entry.status),
                    Some(ListStatus::Current)
                );
                assert_eq!(entry.as_ref().map(|entry| entry.progress), Some(3));
            }
            #[expect(
                clippy::panic,
                reason = "the assertion prints the Entry's actual state"
            )]
            state => panic!("the Entry should be ready: {state:?}"),
        }
    }

    #[gpui_kit::test]
    async fn a_save_finishing_after_a_new_entry_opens_leaves_the_new_detail_alone(
        cx: &mut TestAppContext,
    ) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = Arc::new(FakeServices {
            save_delay: Duration::from_millis(150),
            ..FakeServices::default()
        });
        let store = cx.new(|_| EntryStore::new(services.clone()));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        cx.run_until_parked();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store
                    .draft_mut()
                    .expect("the first Entry is ready")
                    .progress = 3;
            });
            store.update(cx, EntryStore::save);
            // Opening another Entry must not cancel the save, only its writes.
            store.update(cx, |store, cx| store.open(OTHER_ID, cx));
        });
        tokio::time::sleep(Duration::from_millis(250)).await;
        cx.run_until_parked();

        assert_eq!(
            services.saved.lock().expect("the fake's saved lock").len(),
            1
        );
        match store.read_with(cx, |store, _| store.state().clone()) {
            EntryState::Ready { detail, entry, .. } => {
                assert_eq!(detail.summary.anilist_id, OTHER_ID);
                assert!(
                    detail.summary.title.romaji.as_deref() == Some("Media 42001"),
                    "the second Entry's detail is still on screen"
                );
                assert!(entry.is_none(), "the first Entry's list entry did not leak");
            }
            #[expect(
                clippy::panic,
                reason = "the assertion prints the Entry's actual state"
            )]
            state => panic!("the second Entry should be ready: {state:?}"),
        }
    }

    #[gpui_kit::test]
    async fn edits_made_while_a_save_is_in_flight_are_kept(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = Arc::new(FakeServices {
            save_delay: Duration::from_millis(150),
            ..FakeServices::default()
        });
        let store = cx.new(|_| EntryStore::new(services.clone()));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        cx.run_until_parked();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.draft_mut().expect("the Entry is ready").progress = 3;
            });
            store.update(cx, EntryStore::save);
            // The user keeps editing while the save is in flight.
            store.update(cx, |store, _| {
                store.draft_mut().expect("the Entry is ready").progress = 4;
            });
        });
        tokio::time::sleep(Duration::from_millis(250)).await;
        cx.run_until_parked();

        match store.read_with(cx, |store, _| store.state().clone()) {
            EntryState::Ready { entry, draft, .. } => {
                assert_eq!(
                    entry.map(|entry| entry.progress),
                    Some(3),
                    "the save landed"
                );
                assert_eq!(draft.progress, 4, "the newer edit survived the save");
            }
            #[expect(
                clippy::panic,
                reason = "the assertion prints the Entry's actual state"
            )]
            state => panic!("the Entry should be ready: {state:?}"),
        }
    }

    #[gpui_kit::test]
    async fn a_stale_detail_is_replaced_by_its_refresh(cx: &mut TestAppContext) {
        let runtime = runtime();
        let _guard = runtime.enter();
        cx.executor().allow_parking();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = Arc::new(FakeServices {
            refreshed_title: Arc::new(Mutex::new(Some("Fresh".to_owned()))),
            refresh_delay: Duration::from_millis(150),
            ..FakeServices::default()
        });
        let store = cx.new(|_| EntryStore::new(services));
        cx.update(|cx| store.update(cx, |store, cx| store.open(ANILIST_ID, cx)));
        cx.run_until_parked();
        tokio::time::sleep(Duration::from_millis(50)).await;
        cx.run_until_parked();

        match store.read_with(cx, |store, _| store.state().clone()) {
            EntryState::Ready { detail, .. } => {
                assert_eq!(detail.description.as_deref(), Some("stale"));
            }
            #[expect(
                clippy::panic,
                reason = "the assertion prints the Entry's actual state"
            )]
            state => panic!("the Entry should be ready: {state:?}"),
        }

        tokio::time::sleep(Duration::from_millis(200)).await;
        cx.run_until_parked();
        match store.read_with(cx, |store, _| store.state().clone()) {
            EntryState::Ready { detail, .. } => {
                assert_eq!(detail.description.as_deref(), Some("Fresh"));
            }
            #[expect(
                clippy::panic,
                reason = "the assertion prints the Entry's actual state"
            )]
            state => panic!("the Entry should be ready: {state:?}"),
        }
    }
}
