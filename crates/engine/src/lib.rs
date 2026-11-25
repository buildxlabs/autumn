//! Autumn's engine: the composition root of the Tokio-side services (#58).
//!
//! The engine owns the database, the shared HTTP client, the AniList
//! catalog (with its stale-first disk cache) and the active tracking
//! platform (the Local, account-free platform). `autumn` builds the Tokio
//! runtime and starts the engine with [`Engine::new`]; the ui-state stores
//! call its services through [`bridge::run`] (ARCHITECTURE §4). It grows
//! into ARCHITECTURE's engine in Phase 2.

mod chapters;
mod library;
mod playback;
mod settings_store;

use std::{
    collections::HashSet,
    ffi::OsStr,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
};

use ::library::{AutoScan, Library, ScanSummary};
use db::{Db, FileFlag, LocalFile};
use domain::{AppEvent, ExternalId, ListEntry, ListStatus, MediaId, MediaKind, PlaybackControl};
use flume::Sender;
use metadata::{AniList, CatalogCache, HomeLoad, Load, MediaDetail, MediaRelation, MediaSummary};
use net::{Client, PosterCache};
use settings::{AppPaths, LibrarySettings, Settings};
use tokio::runtime::Runtime;
use tracker::{Local, Platform, PlatformError};

pub use library::{
    AnimeList, FillerPolicy, LibraryMediaInfo, LibraryScanner, PlatformCandidates, ScanError,
};
pub use playback::{
    LocalProgressSync, MpvLauncher, PlayRequest, PlaybackError, PlaybackManager, PlaybackSession,
    PlayerLaunch, PlayerLauncher, ProgressSync, SyncError, sidecar_subtitles,
};

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
    /// The active platform exposed by the composition root failed.
    #[error("the active platform call failed: {0}")]
    ActivePlatform(String),
    /// A file operation failed.
    #[error("the file operation failed: {0}")]
    Io(#[from] std::io::Error),
    /// Files could not be moved to the operating system's trash.
    #[error("files could not be moved to the trash: {0}")]
    Trash(String),
    /// A new file name contained a path separator or `..`.
    #[error("invalid file name: {0}")]
    InvalidPath(String),
    /// The destination of a rename already exists.
    #[error("a file already exists at {}", .0.display())]
    AlreadyExists(PathBuf),
    /// The file to rename has no library row, so nothing was changed.
    #[error("{} is not in the library", .0.display())]
    NotInLibrary(PathBuf),
    /// A library scan failed.
    #[error("the library scan failed: {0}")]
    Library(#[from] ::library::LibraryError),
    /// A scan was refused because another one is running.
    #[error("{0}")]
    Scan(#[from] ScanError),
    /// The candidate-source slot was poisoned.
    #[error("the library candidate lock was poisoned")]
    CandidateLock(String),
    /// The watcher state lock was poisoned.
    #[error("the library watcher lock was poisoned")]
    WatcherLock,
    /// The live Library settings lock was poisoned.
    #[error("the library settings lock was poisoned")]
    SettingsLock,
    /// A play request could not start.
    #[error("playback could not start: {0}")]
    Playback(#[from] PlaybackError),
    /// A HUD command arrived with no session to receive it.
    #[error("no playback session is running")]
    NoSession,
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

    /// Starts `request`, stopping the previous session first. The default is
    /// a no-op so stores that only mirror playback events need no fake.
    fn play(&self, _request: &PlayRequest) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Stops the active session. The default is a no-op for mirroring fakes.
    fn stop(&self) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Retries a failed progress sync. The default is a no-op for fakes.
    fn retry_sync(&self, _media: MediaId, _episode: u32) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Pauses or resumes the active session. The default is a no-op for fakes.
    fn set_playback_state(&self, _state: PlaybackControl) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Seeks the active session to `seconds`. The default is a no-op for fakes.
    fn seek(&self, _seconds: f64) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Skips the chapter currently playing. The default is a no-op for fakes.
    fn skip_chapter(&self) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Starts one scan of the folders in the Library settings. A second call
    /// while a scan runs fails with [`ScanError::AlreadyScanning`]. The default
    /// is a no-op for fakes.
    fn start_scan(&self) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// The library files matched to `media`, path ordered.
    fn library_files(&self, _media: MediaId) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Every media with at least one local file.
    fn library_media(&self) -> BoxFuture<Result<Vec<MediaId>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// The newest scan summary, if one has been recorded.
    fn latest_scan_summary(&self) -> BoxFuture<Result<Option<ScanSummary>, EngineError>> {
        Box::pin(async { Ok(None) })
    }

    /// Every local file, so stores can load the library in one call.
    fn all_library_files(&self) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Stored scan summaries, newest first. The default is empty for fakes.
    fn summaries(&self) -> BoxFuture<Result<Vec<ScanSummary>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Media with local files, with their Entry ids and titles.
    fn library_media_info(&self) -> BoxFuture<Result<Vec<LibraryMediaInfo>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Assigns files to AniList media, minting it if needed. The default is a
    /// no-op for fakes.
    fn manual_match(
        &self,
        _paths: &[PathBuf],
        _anilist_id: u32,
        _episode: Option<u32>,
    ) -> BoxFuture<Result<usize, EngineError>> {
        Box::pin(async { Ok(0) })
    }

    /// Marks a file as ignored. The default is a no-op for fakes.
    fn ignore_file(&self, _path: &Path) -> BoxFuture<Result<usize, EngineError>> {
        Box::pin(async { Ok(0) })
    }

    /// Sets or clears a file's lock (the default is a no-op).
    fn set_library_lock(
        &self,
        _path: &Path,
        _flag: FileFlag,
    ) -> BoxFuture<Result<usize, EngineError>> {
        Box::pin(async { Ok(0) })
    }

    /// Sets or clears a file's ignored flag (the default is a no-op).
    fn set_library_ignored(
        &self,
        _path: &Path,
        _flag: FileFlag,
    ) -> BoxFuture<Result<usize, EngineError>> {
        Box::pin(async { Ok(0) })
    }

    /// Renames a file on disk and in the library (the default is a no-op).
    fn rename_library_file(
        &self,
        _from: &Path,
        _to: &Path,
    ) -> BoxFuture<Result<usize, EngineError>> {
        Box::pin(async { Ok(0) })
    }

    /// Moves files to the OS trash and drops their rows (the default is a no-op).
    fn trash_library_files(&self, _paths: &[PathBuf]) -> BoxFuture<Result<usize, EngineError>> {
        Box::pin(async { Ok(0) })
    }
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
    /// The Library tab's current values, shared with scans so saves take
    /// effect without a restart (#127).
    library_settings: Arc<RwLock<LibrarySettings>>,
    library: Library,
    handle: tokio::runtime::Handle,
    events: Arc<Mutex<Option<Sender<AppEvent>>>>,
    scanner: Arc<LibraryScanner>,
    watcher: Arc<Mutex<Option<AutoScan>>>,
    playback: Arc<PlaybackManager>,
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
    pub fn new(
        runtime: &Runtime,
        paths: &AppPaths,
        events: Sender<AppEvent>,
    ) -> Result<Self, EngineError> {
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
        let entries: Arc<dyn AnimeList> = Arc::new(platform.clone());
        let candidates: Arc<dyn library::CandidateSource> = Arc::new(PlatformCandidates::new(
            entries,
            db.clone(),
            catalog.clone(),
            client.clone(),
            poster_cache.clone(),
        ));
        let events_slot = Arc::new(Mutex::new(None));
        let library_settings = Arc::new(RwLock::new(settings.library.clone()));
        let scanner = Arc::new(LibraryScanner::with_hydration(
            db.clone(),
            candidates,
            Arc::clone(&library_settings),
            client.clone(),
            poster_cache.clone(),
            Arc::clone(&events_slot),
            runtime.handle().clone(),
        ));
        let playback = Arc::new(PlaybackManager::new(
            db.clone(),
            catalog.clone(),
            Arc::new(LocalProgressSync::new(db.clone())),
            events,
            &settings.playback,
            paths.cache_dir().to_path_buf(),
        ));
        Ok(Self {
            db,
            client,
            poster_cache,
            catalog,
            platform,
            settings,
            library_settings,
            library,
            handle: runtime.handle().clone(),
            events: events_slot,
            scanner,
            watcher: Arc::new(Mutex::new(None)),
            playback,
        })
    }

    /// Attaches the engine-to-UI event channel used for scan progress and the
    /// library-changed event.
    #[must_use]
    pub fn with_events(self, events: Sender<AppEvent>) -> Self {
        if let Ok(mut slot) = self.events.lock() {
            *slot = Some(events);
        }
        self
    }

    /// Uses `entries` as the active platform's list when building candidates.
    /// The binary calls this after login so scans follow ADR 0004.
    pub fn set_anime_list(&self, entries: Arc<dyn AnimeList>) {
        let candidates: Arc<dyn library::CandidateSource> = Arc::new(PlatformCandidates::new(
            entries,
            self.db.clone(),
            self.catalog.clone(),
            self.client.clone(),
            self.poster_cache.clone(),
        ));
        self.scanner.set_candidates(candidates);
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
        {
            let mut current = self
                .library_settings
                .write()
                .map_err(|_| EngineError::SettingsLock)?;
            *current = settings.library.clone();
        }
        let mut watcher = self.watcher.lock().map_err(|_| EngineError::WatcherLock)?;
        let folders = &settings.library.folders;
        if !settings.library.auto_scan || folders.is_empty() {
            if let Some(previous) = watcher.take() {
                previous.stop();
            }
            return Ok(());
        }
        let scanner = Arc::clone(&self.scanner);
        let handle = self.handle.clone();
        // Build the replacement first: a failed restart must not leave the
        // engine with no watcher at all. A change in one folder still rescans
        // every folder, because a partial scan would drop the others' rows.
        let auto_scan = AutoScan::start(folders, &handle, move |_changed| {
            scanner.request_rescan();
        })?;
        if let Some(previous) = watcher.replace(auto_scan) {
            previous.stop();
        }
        Ok(())
    }

    /// Replaces the playback manager's progress sync with the active platform
    /// gateway, so watched episodes reach whichever platform is in use.
    pub fn set_progress_sync(&self, sync: Arc<dyn ProgressSync>) {
        self.playback.set_sync(sync);
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

    /// The playback manager that owns the one active session.
    #[must_use]
    pub fn playback(&self) -> &Arc<PlaybackManager> {
        &self.playback
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

    fn play(&self, request: &PlayRequest) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        let request = request.clone();
        Box::pin(async move { playback.play(request).await.map_err(EngineError::from) })
    }

    fn stop(&self) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            playback.stop().await;
            Ok(())
        })
    }

    fn retry_sync(&self, media: MediaId, episode: u32) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            playback.retry_sync(media, episode).await;
            Ok(())
        })
    }

    fn set_playback_state(&self, state: PlaybackControl) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            if playback.set_playback_state(state) {
                Ok(())
            } else {
                Err(EngineError::NoSession)
            }
        })
    }

    fn seek(&self, seconds: f64) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            if playback.seek(seconds) {
                Ok(())
            } else {
                Err(EngineError::NoSession)
            }
        })
    }

    fn skip_chapter(&self) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            if playback.skip_chapter() {
                Ok(())
            } else {
                Err(EngineError::NoSession)
            }
        })
    }

    fn start_scan(&self) -> BoxFuture<Result<(), EngineError>> {
        let scanner = Arc::clone(&self.scanner);
        Box::pin(async move {
            scanner.start()?;
            Ok(())
        })
    }

    fn library_files(&self, media: MediaId) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            library::files_for(&db, media)
                .await
                .map_err(EngineError::from)
        })
    }

    fn library_media(&self) -> BoxFuture<Result<Vec<MediaId>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move { library::matched_media(&db).await.map_err(EngineError::from) })
    }

    fn latest_scan_summary(&self) -> BoxFuture<Result<Option<ScanSummary>, EngineError>> {
        let library = self.library.clone();
        Box::pin(async move { library.latest_summary().await.map_err(EngineError::from) })
    }

    fn all_library_files(&self) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move { db.local_file().all().await.map_err(EngineError::from) })
    }

    fn summaries(&self) -> BoxFuture<Result<Vec<ScanSummary>, EngineError>> {
        let library = self.library.clone();
        Box::pin(async move { library.summaries().await.map_err(EngineError::from) })
    }

    fn library_media_info(&self) -> BoxFuture<Result<Vec<LibraryMediaInfo>, EngineError>> {
        let (db, catalog) = (self.db.clone(), self.catalog.clone());
        Box::pin(async move {
            library::media_info(&db, &catalog)
                .await
                .map_err(EngineError::from)
        })
    }

    fn manual_match(
        &self,
        paths: &[PathBuf],
        anilist_id: u32,
        episode: Option<u32>,
    ) -> BoxFuture<Result<usize, EngineError>> {
        let library = self.library.clone();
        let paths = paths.to_vec();
        let events = self
            .events
            .lock()
            .map_or_else(|_| None, |slot| slot.clone());
        Box::pin(async move {
            let changed = library
                .manual_match(&paths, anilist_id, episode)
                .await
                .map_err(EngineError::from)?;
            if let Some(events) = events {
                let _ = events.send(AppEvent::LibraryChanged);
            }
            Ok(changed)
        })
    }

    fn ignore_file(&self, path: &Path) -> BoxFuture<Result<usize, EngineError>> {
        let library = self.library.clone();
        let path = path.to_path_buf();
        let events = self
            .events
            .lock()
            .map_or_else(|_| None, |slot| slot.clone());
        Box::pin(async move {
            let changed = library
                .ignore_file(&path)
                .await
                .map_err(EngineError::from)?;
            // Other views (Library, Entry) drop the file, as after a match.
            if let Some(events) = events {
                let _ = events.send(AppEvent::LibraryChanged);
            }
            Ok(changed)
        })
    }

    fn set_library_lock(
        &self,
        path: &Path,
        flag: FileFlag,
    ) -> BoxFuture<Result<usize, EngineError>> {
        let library = self.library.clone();
        let events = self.events_slot();
        let path = path.to_path_buf();
        Box::pin(async move {
            let changed = match flag {
                FileFlag::Set => library.lock_file(&path).await,
                FileFlag::Clear => library.unlock_file(&path).await,
                _ => Ok(0),
            }?;
            if let Some(events) = events {
                let _ = events.send(AppEvent::LibraryChanged);
            }
            Ok(changed)
        })
    }

    fn set_library_ignored(
        &self,
        path: &Path,
        flag: FileFlag,
    ) -> BoxFuture<Result<usize, EngineError>> {
        let library = self.library.clone();
        let events = self.events_slot();
        let path = path.to_path_buf();
        Box::pin(async move {
            let changed = match flag {
                FileFlag::Set => library.ignore_file(&path).await,
                FileFlag::Clear => library.unignore_file(&path).await,
                _ => Ok(0),
            }?;
            if let Some(events) = events {
                let _ = events.send(AppEvent::LibraryChanged);
            }
            Ok(changed)
        })
    }

    fn rename_library_file(&self, from: &Path, to: &Path) -> BoxFuture<Result<usize, EngineError>> {
        let db = self.db.clone();
        let events = self.events_slot();
        let handle = self.handle.clone();
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        Box::pin(async move {
            // The DB update and the disk rename must both run or both roll
            // back. A spawned task runs to the end even if the UI drops this
            // future between the two steps.
            let changed = handle
                .spawn(rename_file(db, from, to))
                .await
                .map_err(|error| EngineError::Io(std::io::Error::other(error)))??;
            if let Some(events) = events {
                let _ = events.send(AppEvent::LibraryChanged);
            }
            Ok(changed)
        })
    }

    fn trash_library_files(&self, paths: &[PathBuf]) -> BoxFuture<Result<usize, EngineError>> {
        let db = self.db.clone();
        let events = self.events_slot();
        let paths = paths.to_vec();
        Box::pin(async move {
            let to_trash = paths.clone();
            tokio::task::spawn_blocking(move || trash::delete_all(&to_trash))
                .await
                .map_err(|error| EngineError::Trash(error.to_string()))?
                .map_err(|error| EngineError::Trash(error.to_string()))?;
            let changed = usize::try_from(db.local_file().delete(&paths).await?)
                .map_err(|_| EngineError::Io(std::io::Error::other("row count overflow")))?;
            if let Some(events) = events {
                let _ = events.send(AppEvent::LibraryChanged);
            }
            Ok(changed)
        })
    }
}

/// Renames one library file in the database, then on disk, rolling the row
/// back when the disk rename fails.
async fn rename_file(db: Db, from: PathBuf, to: PathBuf) -> Result<usize, EngineError> {
    let Some(name) = to.file_name().and_then(OsStr::to_str) else {
        return Err(EngineError::InvalidPath(
            "the new name is missing".to_owned(),
        ));
    };
    if to.parent() != from.parent() || !valid_file_name(name) {
        return Err(EngineError::InvalidPath(
            "renames stay in the file's folder and reject separators or '..'".to_owned(),
        ));
    }
    // A case-only rename on a case-insensitive filesystem finds the source
    // itself at the target; only a different file blocks the rename.
    //
    // This check and the rename below can race with another process creating
    // `to` in between: std has no portable no-replace rename (renameat2's
    // RENAME_NOREPLACE, renamex_np's RENAME_EXCL), so that window stays open.
    if let Ok(existing) = tokio::fs::symlink_metadata(&to).await
        && !same_file(&from, &to, &existing).await
    {
        return Err(EngineError::AlreadyExists(to));
    }
    // Update the row first: a failed DB write must never be followed by a
    // disk rename that loses the original file.
    let changed = db.local_file().rename(&from, &to).await?;
    if changed == 0 {
        return Err(EngineError::NotInLibrary(from));
    }
    if let Err(error) = tokio::fs::rename(&from, &to).await {
        if let Err(rollback) = db.local_file().rename(&to, &from).await {
            tracing::error!(
                %rollback,
                from = %from.display(),
                to = %to.display(),
                "a failed rename left the library row at the new path"
            );
        }
        return Err(error.into());
    }
    usize::try_from(changed)
        .map_err(|_| EngineError::Io(std::io::Error::other("row count overflow")))
}

/// Whether `to`, whose metadata is `existing`, is the same file as `from`.
async fn same_file(from: &Path, to: &Path, existing: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;

        let _ = to;
        tokio::fs::symlink_metadata(from)
            .await
            .is_ok_and(|source| source.dev() == existing.dev() && source.ino() == existing.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = existing;
        match (
            tokio::fs::canonicalize(from).await,
            tokio::fs::canonicalize(to).await,
        ) {
            (Ok(source), Ok(target)) => source == target,
            _ => false,
        }
    }
}

/// Whether `name` is a safe file name for an inline rename.
#[must_use]
pub(crate) fn valid_file_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains('/') && !name.contains('\\')
}

impl Engine {
    /// The current event sender, when the channel has been attached.
    fn events_slot(&self) -> Option<Sender<AppEvent>> {
        self.events
            .lock()
            .map_or_else(|_| None, |slot| slot.clone())
    }

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
    use std::{fs, path::PathBuf, time::SystemTime};

    use domain::{AdvancedScores, ExternalId, FuzzyDate, ListStatus, MediaKind, Score};
    use settings::{AppPaths, Settings};
    use tempfile::{TempDir, tempdir};
    use tokio::runtime::Builder;

    use super::{Engine, EngineError, EngineServices, valid_file_name};

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
        let (events, _receiver) = flume::unbounded();
        let engine = Engine::new(&runtime, &paths, events).expect("the engine");
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

        let (events, _receiver) = flume::unbounded();
        {
            let engine = Engine::new(&runtime, &paths, events.clone()).expect("the first engine");
            let media = runtime
                .block_on(engine.mint(MediaKind::Anime, &ids))
                .expect("the media identity");
            runtime
                .block_on(engine.save_entry(&entry(media)))
                .expect("the Local list save");
        } // Drop the engine: the next one opens the same database afresh.

        let reopened = Engine::new(&runtime, &paths, events).expect("the restarted engine");
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

    #[test]
    fn inline_rename_rejects_path_tricks() {
        assert!(valid_file_name("episode 01.mkv"));
        assert!(!valid_file_name(""));
        assert!(!valid_file_name("."));
        assert!(!valid_file_name(".."));
        assert!(!valid_file_name("../ep.mkv"));
        assert!(!valid_file_name("sub/ep.mkv"));
        assert!(!valid_file_name("sub\\ep.mkv"));
    }

    #[test]
    fn a_rename_never_overwrites_an_existing_file() {
        let dir = tempdir().expect("a temp directory");
        let paths = paths(&dir);
        let folder = dir.path().join("shows");
        fs::create_dir_all(&folder).expect("the library folder");
        let from = folder.join("a.mkv");
        let to = folder.join("b.mkv");
        fs::write(&from, b"original").expect("the source file");
        fs::write(&to, b"keep me").expect("the existing target");
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let (events, _receiver) = flume::unbounded();
        let engine = Engine::new(&runtime, &paths, events).expect("the engine");

        assert!(matches!(
            runtime.block_on(engine.rename_library_file(&from, &to)),
            Err(EngineError::AlreadyExists(_))
        ));
        assert_eq!(fs::read(&from).expect("the source survives"), b"original");
        assert_eq!(fs::read(&to).expect("the target survives"), b"keep me");

        assert!(matches!(
            runtime.block_on(engine.rename_library_file(&from, &PathBuf::from("../escaped.mkv"))),
            Err(EngineError::InvalidPath(_))
        ));
    }

    fn library_row(path: &std::path::Path) -> db::LocalFile {
        db::LocalFile {
            path: path.to_path_buf(),
            size: 0,
            modified: 0,
            title: None,
            group: None,
            season: None,
            episode: None,
            kind: "episode".to_owned(),
            media_id: None,
            confidence: None,
            locked: false,
            ignored: false,
            filler: false,
            shelved: false,
        }
    }

    fn rename_fixture() -> (TempDir, tokio::runtime::Runtime, Engine, PathBuf) {
        let dir = tempdir().expect("a temp directory");
        let paths = paths(&dir);
        let folder = dir.path().join("shows");
        fs::create_dir_all(&folder).expect("the library folder");
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("a test runtime");
        let (events, _receiver) = flume::unbounded();
        let engine = Engine::new(&runtime, &paths, events).expect("the engine");
        (dir, runtime, engine, folder)
    }

    #[test]
    fn a_rename_without_a_library_row_leaves_the_disk_alone() {
        let (_dir, runtime, engine, folder) = rename_fixture();
        let from = folder.join("a.mkv");
        let to = folder.join("b.mkv");
        fs::write(&from, b"original").expect("the source file");

        assert!(matches!(
            runtime.block_on(engine.rename_library_file(&from, &to)),
            Err(EngineError::NotInLibrary(_))
        ));
        assert!(from.exists(), "the file stays where it was");
        assert!(!to.exists());
    }

    #[test]
    fn a_rename_moves_the_file_and_its_row() {
        let (_dir, runtime, engine, folder) = rename_fixture();
        let from = folder.join("ep.mkv");
        let to = folder.join("EP.mkv");
        fs::write(&from, b"original").expect("the source file");
        runtime
            .block_on(engine.db().local_file().sync(&[library_row(&from)]))
            .expect("the library row");

        // A case-only rename: on a case-insensitive filesystem the target
        // already "exists" as the source itself.
        assert_eq!(
            runtime
                .block_on(engine.rename_library_file(&from, &to))
                .expect("the rename succeeds"),
            1
        );
        let names: Vec<_> = fs::read_dir(&folder)
            .expect("the folder lists")
            .filter_map(|entry| entry.ok().map(|entry| entry.file_name()))
            .collect();
        assert_eq!(names, ["EP.mkv"], "the file was renamed on disk");
        let rows = runtime
            .block_on(engine.db().local_file().all())
            .expect("the rows read");
        assert_eq!(
            rows.iter().map(|row| row.path.clone()).collect::<Vec<_>>(),
            [to]
        );
    }
}
