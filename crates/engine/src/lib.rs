//! Autumn's engine: the composition root of the Tokio-side services (#58).
//!
//! The engine owns the database, the shared HTTP client, the AniList
//! catalog (with its stale-first disk cache) and the active tracking
//! platform (the Local, account-free platform). `autumn` builds the Tokio
//! runtime and starts the engine with [`Engine::new`]; the ui-state stores
//! call its services through [`bridge::run`] (ARCHITECTURE §4). It grows
//! into ARCHITECTURE's engine in Phase 2.

mod settings_store;

use std::{
    collections::HashSet,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use db::Db;
use domain::{AppEvent, ExternalId, ListEntry, ListStatus, MediaId, MediaKind};
use flume::Sender;
use library::{AutoScan, Library};
use metadata::{AniList, CatalogCache, HomeLoad, Load, MediaDetail, MediaRelation, MediaSummary};
use net::{Client, PosterCache};
use settings::{AppPaths, Settings};
use tokio::runtime::Runtime;
use tracker::{Local, Platform, PlatformError};

/// A boxed engine-service call: object-safe, `Send` and `'static`.
pub type BoxFuture<T> = Pin<Box<dyn Future<Output = T> + Send + 'static>>;

/// A list entry with the card text already on disk: `None` when the media
/// was never opened, so no summary is cached.
pub type ListedEntry = (ListEntry, Option<MediaSummary>);

/// Why an engine service call failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum EngineError {
    /// The database could not be opened.
    #[error("the database could not be opened: {0}")]
    Db(#[from] db::DbError),
    /// The settings could not be loaded, or a stored row is invalid.
    #[error("the settings could not be loaded: {0}")]
    Settings(#[from] settings::SettingsError),
    /// The shared HTTP client could not be built.
    #[error("the HTTP client could not be built: {0}")]
    Client(#[from] net::Error),
    /// A metadata request failed.
    #[error("the metadata request failed: {0}")]
    Metadata(#[from] metadata::Error),
    /// A platform call failed.
    #[error("the platform call failed: {0}")]
    Platform(#[from] PlatformError),
    /// A library scan failed.
    #[error("the library scan failed: {0}")]
    Library(#[from] library::LibraryError),
    /// The watcher state lock was poisoned.
    #[error("the library watcher lock was poisoned")]
    WatcherLock,
}

/// The engine's services: what the Anime Entry and Home screens call through
/// [`bridge::run`]. Boxed futures make the trait object-safe: stores hold
/// `Arc<dyn EngineServices>`, and tests fake it.
pub trait EngineServices: Send + Sync + 'static {
    /// The Entry page's detail, stale-first (24 h TTL).
    fn detail(&self, anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>>;
    /// The media identity behind the Entry: never mints.
    fn local_media(
        &self,
        kind: MediaKind,
        ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>>;
    /// The media's entry on the active platform, if it has one.
    fn local_entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>>;
    /// Saves the editor draft on the active platform.
    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>>;
    /// Finds or mints the media identity: call when the user first records a fact.
    fn mint(&self, kind: MediaKind, ids: &[ExternalId]) -> BoxFuture<Result<MediaId, EngineError>>;
    /// The Home catalog load, stale-first (6 h TTL).
    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>>;
    /// The active platform's entries for `kind`, most recently updated
    /// first, each with its cached summary. Reads the cache only: no
    /// network request.
    fn collection(&self, kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>>;
    /// The sequels of the user's completed `kind` entries that are not on
    /// their list, from the cached Entry relations. Reads the cache only:
    /// no network request.
    fn missed_sequels(&self, kind: MediaKind) -> BoxFuture<Result<Vec<MediaSummary>, EngineError>>;
}

/// The engine: owns the database, the shared client, the catalog cache and
/// the active tracking platform.
#[derive(Debug, Clone)]
pub struct Engine {
    db: Db,
    client: Client,
    poster_cache: PosterCache,
    catalog: CatalogCache,
    platform: Local,
    settings: Settings,
    library: Library,
    events: Option<Sender<AppEvent>>,
    watcher: Arc<Mutex<Option<AutoScan>>>,
}

/// The loopback host of debug fetches; `fetch_status::observe` watches its rate limits.
pub const DEBUG_HOST: &str = "127.0.0.1";

impl Engine {
    /// Opens the database at `paths.database_file()`, imports the Phase 0
    /// `settings.json` once and loads the settings, builds the shared client
    /// (with the debug host registered), and starts the Local platform.
    ///
    /// # Errors
    ///
    /// - [`EngineError::Db`] when the database can't be opened.
    /// - [`EngineError::Settings`] when the settings can't be loaded or a row is invalid.
    /// - [`EngineError::Client`] when the client can't be built.
    pub fn new(runtime: &Runtime, paths: &AppPaths) -> Result<Self, EngineError> {
        let db = runtime.block_on(Db::open(&paths.database_file()))?;
        let settings = runtime.block_on(settings_store::load(&db, &paths.settings_file()))?;
        let builder = Client::builder().anilist();
        #[cfg(debug_assertions)]
        let builder = builder.host(DEBUG_HOST, 25, 5, net::HostKind::AniList);
        let client = builder.build()?;
        let poster_cache = PosterCache::new(paths.cache_dir())?;
        let catalog = CatalogCache::new(AniList::new(client.clone()), poster_cache.clone());
        let platform = Local::new(db.clone());
        let library = Library::new(db.clone());
        Ok(Self {
            db,
            client,
            poster_cache,
            catalog,
            platform,
            settings,
            library,
            events: None,
            watcher: Arc::new(Mutex::new(None)),
        })
    }

    /// Attaches the engine-to-UI event channel used for scan progress and the
    /// library-changed event.
    #[must_use]
    pub fn with_events(mut self, events: Sender<AppEvent>) -> Self {
        self.events = Some(events);
        self
    }

    /// The library service, for explicit scans from screens and the watcher.
    #[must_use]
    pub fn library(&self) -> &Library {
        &self.library
    }

    /// Whether an auto-scan watcher is currently running.
    #[must_use]
    pub fn auto_scan_running(&self) -> bool {
        self.watcher.lock().is_ok_and(|watcher| watcher.is_some())
    }

    /// Starts, restarts or stops the auto-scan watcher to match the Library
    /// settings. Calling this after a settings save is the live reload.
    ///
    /// # Errors
    ///
    /// [`EngineError::WatcherLock`] if the watcher state lock is poisoned, or
    /// [`EngineError::Library`] if a folder cannot be watched.
    pub fn apply_library_settings(&self, settings: &Settings) -> Result<(), EngineError> {
        let mut watcher = self.watcher.lock().map_err(|_| EngineError::WatcherLock)?;
        if let Some(previous) = watcher.take() {
            previous.stop();
        }
        let folders = &settings.library.folders;
        if !settings.library.auto_scan || folders.is_empty() {
            return Ok(());
        }
        let engine = self.clone();
        let events = self.events.clone();
        let auto_scan = AutoScan::start(folders, move |changed| {
            let engine = engine.clone();
            let events = events.clone();
            tokio::spawn(async move {
                engine.run_auto_scan(&changed, events).await;
            });
        })?;
        *watcher = Some(auto_scan);
        Ok(())
    }

    /// Runs one auto-scan and publishes its progress and completion.
    async fn run_auto_scan(
        &self,
        folders: &[std::path::PathBuf],
        events: Option<Sender<AppEvent>>,
    ) {
        // Candidates arrive from the metadata engine in #127; until then the
        // scan still stores and shelves files, just without a match.
        let last = Arc::new(Mutex::new(
            Instant::now()
                .checked_sub(Duration::from_secs(1))
                .unwrap_or(Instant::now()),
        ));
        let result = self
            .library
            .scan(folders, &[], |progress| {
                let Some(events) = &events else {
                    return;
                };
                let Ok(mut last) = last.lock() else {
                    return;
                };
                if last.elapsed() < Duration::from_millis(250) {
                    return;
                }
                *last = Instant::now();
                let _ = events.send(AppEvent::LibraryScanProgress {
                    files_walked: progress.files_walked,
                    files_matched: progress.files_matched,
                });
            })
            .await;
        match result {
            Ok(_) => {
                if let Some(events) = events {
                    let _ = events.send(AppEvent::LibraryChanged);
                }
            }
            Err(error) => tracing::warn!(%error, "auto-scan failed"),
        }
    }

    /// The app database.
    #[must_use]
    pub fn db(&self) -> &Db {
        &self.db
    }

    /// The settings, loaded when the engine started.
    #[must_use]
    pub fn settings(&self) -> &Settings {
        &self.settings
    }

    /// The shared HTTP client.
    #[must_use]
    pub fn client(&self) -> Client {
        self.client.clone()
    }

    /// The on-disk poster cache.
    #[must_use]
    pub fn poster_cache(&self) -> PosterCache {
        self.poster_cache.clone()
    }
}

impl EngineServices for Engine {
    fn detail(&self, anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { catalog.media(anilist_id).await.map_err(EngineError::from) })
    }

    fn local_media(
        &self,
        kind: MediaKind,
        ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        let db = self.db.clone();
        let ids = ids.to_vec();
        Box::pin(async move { db.media().find(kind, &ids).await.map_err(EngineError::from) })
    }

    fn local_entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.local_list()
                .entry(media)
                .await
                .map_err(EngineError::from)
        })
    }

    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        let platform = self.platform.clone();
        let entry = entry.clone();
        Box::pin(async move { platform.save_entry(&entry).await.map_err(EngineError::from) })
    }

    fn mint(&self, kind: MediaKind, ids: &[ExternalId]) -> BoxFuture<Result<MediaId, EngineError>> {
        let db = self.db.clone();
        let ids = ids.to_vec();
        Box::pin(async move {
            db.media()
                .find_or_mint(kind, &ids)
                .await
                .map(|minted| minted.media)
                .map_err(EngineError::from)
        })
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { catalog.home().await.map_err(EngineError::from) })
    }

    fn collection(&self, kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        let engine = self.clone();
        Box::pin(async move {
            let mut listed = Vec::new();
            for entry in engine.entries(kind).await? {
                let summary = match engine.anilist_id(entry.media).await? {
                    Some(id) => engine
                        .catalog
                        .cached_media(id)
                        .await
                        .map(|cached| cached.value.summary),
                    None => None,
                };
                listed.push((entry, summary));
            }
            Ok(listed)
        })
    }

    fn missed_sequels(&self, kind: MediaKind) -> BoxFuture<Result<Vec<MediaSummary>, EngineError>> {
        let engine = self.clone();
        Box::pin(async move {
            let entries = engine.entries(kind).await?;
            let mut on_list = HashSet::new();
            let mut completed = Vec::new();
            for entry in &entries {
                if let Some(id) = engine.anilist_id(entry.media).await? {
                    on_list.insert(id);
                    if entry.status == ListStatus::Completed {
                        completed.push(id);
                    }
                }
            }
            let mut sequels = Vec::new();
            for id in completed {
                let Some(cached) = engine.catalog.cached_media(id).await else {
                    continue;
                };
                for relation in cached.value.relations {
                    if relation.relation == Some(MediaRelation::Sequel)
                        && on_list.insert(relation.media.anilist_id)
                    {
                        sequels.push(relation.media);
                    }
                }
            }
            Ok(sequels)
        })
    }
}

impl Engine {
    /// The active platform's `kind` entries, most recently updated first.
    async fn entries(&self, kind: MediaKind) -> Result<Vec<ListEntry>, EngineError> {
        let collection = self.platform.collection(kind).await?;
        let mut entries: Vec<ListEntry> = collection
            .lists
            .into_iter()
            .flat_map(|list| list.entries)
            .collect();
        entries.sort_by_key(|entry| std::cmp::Reverse(entry.updated_at));
        Ok(entries)
    }

    /// The AniList id `media` is known by, if any.
    async fn anilist_id(&self, media: MediaId) -> Result<Option<u32>, EngineError> {
        let ids = self.db.media().external_ids(media).await?;
        Ok(ids.into_iter().find_map(|id| match id {
            ExternalId::AniList(id) => Some(id),
            _ => None,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use domain::{AdvancedScores, ExternalId, FuzzyDate, ListStatus, MediaKind, Score};
    use settings::{AppPaths, Settings};
    use tempfile::{TempDir, tempdir};
    use tokio::runtime::Builder;

    use super::{Engine, EngineServices};

    fn paths(dir: &TempDir) -> AppPaths {
        let paths = AppPaths::new(
            dir.path().join("config"),
            dir.path().join("data"),
            dir.path().join("cache"),
        );
        paths.create_dirs().expect("the test directories");
        paths
    }

    fn entry(media: domain::MediaId) -> domain::ListEntry {
        domain::ListEntry {
            media,
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
            created_at: SystemTime::now(),
            updated_at: SystemTime::now(),
        }
    }

    #[test]
    fn turning_auto_scan_off_stops_the_watcher_without_a_restart() {
        let dir = tempdir().expect("a temp directory");
        let paths = paths(&dir);
        let folder = dir.path().join("shows");
        std::fs::create_dir_all(&folder).expect("the library folder");
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let _entered = runtime.enter();
        let engine = Engine::new(&runtime, &paths).expect("the engine");
        let mut settings = Settings::default();
        settings.library.folders = vec![folder];
        settings.library.auto_scan = true;

        engine
            .apply_library_settings(&settings)
            .expect("the watcher starts with auto-scan on");
        assert!(engine.auto_scan_running());

        settings.library.auto_scan = false;
        engine
            .apply_library_settings(&settings)
            .expect("the watcher stops");
        assert!(!engine.auto_scan_running());
    }

    #[test]
    fn a_local_list_entry_survives_an_engine_restart() {
        let dir = tempfile::tempdir().expect("a temp directory");
        let paths = paths(&dir);
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let ids = [ExternalId::AniList(154_587)];

        {
            let engine = Engine::new(&runtime, &paths).expect("the first engine");
            let media = runtime
                .block_on(engine.mint(MediaKind::Anime, &ids))
                .expect("the media identity");
            runtime
                .block_on(engine.save_entry(&entry(media)))
                .expect("the Local list save");
        } // Drop the engine: the next one opens the same database afresh.

        let reopened = Engine::new(&runtime, &paths).expect("the restarted engine");
        let media = runtime
            .block_on(reopened.local_media(MediaKind::Anime, &ids))
            .expect("the cross-reference lookup")
            .expect("the media is still known");
        let saved = runtime
            .block_on(reopened.local_entry(media))
            .expect("the Local entry lookup")
            .expect("the list entry is still there");
        assert_eq!(saved.status, ListStatus::Current, "Watching survived");
        assert_eq!(saved.progress, 3, "progress survived");

        // Home reads the same entry back through the active platform.
        let home = runtime
            .block_on(reopened.collection(MediaKind::Anime))
            .expect("the Home collection");
        assert_eq!(
            home.iter()
                .map(|(entry, _)| (entry.media, entry.progress))
                .collect::<Vec<_>>(),
            [(media, 3)],
            "the show is on Home after the restart"
        );
        assert!(
            home.first().is_some_and(|(_, summary)| summary.is_none()),
            "never opened, so no summary is cached; the read made no request"
        );
    }
}
