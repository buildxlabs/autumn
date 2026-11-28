//! Autumn's engine: the composition root of the Tokio-side services (#58).
//!
//! The engine owns the database, the shared HTTP client, the AniList
//! catalog (with its stale-first disk cache) and the active tracking
//! platform (the Local, account-free platform). `autumn` builds the Tokio
//! runtime and starts the engine with [`Engine::new`]; the ui-state stores
//! call its services through [`bridge::run`] (ARCHITECTURE §4). It grows
//! into ARCHITECTURE's engine in Phase 2.

mod auto_select;
mod chapters;
mod extension_lanes;
mod incoming;
mod library;
pub mod network;
#[cfg(test)]
mod phase2_exit;
#[cfg(test)]
mod phase3_exit;
mod playback;
mod settings_store;
#[cfg(test)]
mod test_seeder;
mod torrent_list;
mod torrent_streams;

use std::{
    collections::HashSet,
    ffi::OsStr,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use ::library::{AutoScan, Library, ScanSummary};
use db::{Continuity, Db, FileFlag, LocalFile, ResumePoint};
use domain::{
    AppEvent, ExtensionId, ExternalId, InfoHash, ListEntry, ListStatus, MediaId, MediaKind,
    PlaybackControl, StreamSource, TorrentFileRow, TorrentLink, TorrentListRow, TorrentOrigin,
};
use extension_lanes::{Change, ExtensionLanes};
use flume::Sender;
use metadata::{
    Aging, AiringEpisode, AniList, AniZip, CatalogCache, HomeLoad, Load, Media as EpisodeMetadata,
    MediaDetail, MediaRelation, MediaSummary,
};
use net::{Client, PosterCache};
use settings::{AppPaths, LibrarySettings, Settings};
use tokio::runtime::Runtime;
use tracker::{Local, Platform, PlatformError};

pub use incoming::{
    IncomingAdded, IncomingError, IncomingFile, IncomingSource, MagnetLink, MatchCandidate,
    MatchReview, MatchSuggestion, TorrentFileSource, add_incoming, cancel_incoming,
    commit_incoming, episode_suggestion, match_review, match_titles, parse_magnet,
    parse_torrent_file, pick_episode_file,
};
// `MatchReview`, `IncomingSource`, `IncomingAdded` are referenced unqualified above.
pub use auto_select::{
    AniZipEpisodeIds, AutoSelectError, AutoSelectProgress, AutoSelectRequest, AutoSelection,
    AutoSelector, EpisodeIds, FILE_LIST_TIMEOUT, FileListError, FileLister, SessionFiles,
    TorrentSearch, VERIFY_CANDIDATES,
};
pub use extension_lanes::ExtensionSummary;
pub use library::{
    AnimeList, FillerPolicy, LibraryMediaInfo, LibraryScanner, PlatformCandidates, ScanError,
};
pub use playback::{
    AniZipAiring, EpisodeAiring, LocalProgressSync, MIN_RESUME, MpvLauncher, PlayRequest,
    PlaybackError, PlaybackFileKind, PlaybackManager, PlaybackSession, PlayerLaunch,
    PlayerLauncher, Preloaded, ProgressSync, SyncError, sidecar_subtitles,
};
pub use torrent_streams::{StreamOrigin, TorrentStreams};

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
    /// An incoming-torrent operation failed (#223).
    #[error("incoming torrent failed: {0}")]
    Incoming(String),
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
    /// The embedded torrent session failed an operation.
    #[error("the torrent session failed: {0}")]
    Torrent(#[from] torrent::Error),
    /// An extension could not be listed, enabled, disabled or uninstalled,
    /// for example because no installed extension has the id.
    #[error("the extension change failed: {0}")]
    Extension(#[from] extensions::PackageError),
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

    /// Plays the next episode now, during its countdown. The default is a
    /// no-op for fakes.
    fn play_next_now(&self) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Cancels the next-episode countdown. The default is a no-op for fakes.
    fn cancel_auto_next(&self) -> BoxFuture<Result<(), EngineError>> {
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

    /// Every saved resume position, newest first. The default is empty for
    /// fakes that don't exercise continuity.
    fn resume_points(&self) -> BoxFuture<Result<Vec<ResumePoint>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// The airing schedule, stale-first. This is the only AniList request a
    /// Home library widget makes; the default is empty for fakes.
    fn airing_schedule(&self) -> BoxFuture<Result<Load<Vec<AiringEpisode>>, EngineError>> {
        Box::pin(async {
            Ok(Load {
                cached: metadata::Cached {
                    value: Vec::new(),
                    updated_at: std::time::SystemTime::UNIX_EPOCH,
                    offline: false,
                },
                refresh: None,
            })
        })
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

    /// `AniZip`'s episode titles and thumbnails for AniList `anilist_id`,
    /// from the disk cache while fresh. The default has none, for fakes.
    fn episode_metadata(
        &self,
        _anilist_id: u32,
    ) -> BoxFuture<Result<Option<EpisodeMetadata>, EngineError>> {
        Box::pin(async { Ok(None) })
    }

    /// Runs auto-select for `request` with `profile`, reporting
    /// [`AutoSelectProgress`] to `progress`. The default is a no-op failure
    /// for fakes; [`Engine`] runs [`AutoSelector::select_with_progress`].
    fn auto_select(
        &self,
        _request: AutoSelectRequest,
        _profile: settings::AutoSelectProfile,
        _progress: Box<dyn FnMut(AutoSelectProgress) + Send>,
    ) -> BoxFuture<Result<AutoSelection, AutoSelectError>> {
        Box::pin(async {
            Err(AutoSelectError::IndexerError(search::Error::Transport {
                indexer: search::Indexer::Nyaa,
                message: "auto-select is unavailable".to_owned(),
            }))
        })
    }

    /// A raw indexer search on `lanes`: no profile, no file list (#181 Q13).
    /// Availability badges pass [`search::LaneFilter::BuiltIn`]. The default
    /// fails for fakes that do not exercise badges.
    fn search_torrents(
        &self,
        _request: search::SearchRequest,
        _lanes: search::LaneFilter,
    ) -> BoxFuture<Result<search::SearchOutcome, search::Error>> {
        Box::pin(async { Err(search::Error::AllLanesFailed) })
    }

    /// Stored availability badge results for `media` (#251). The default is
    /// empty for fakes that do not exercise badges.
    fn availability(
        &self,
        _media: i64,
    ) -> BoxFuture<Result<Vec<db::AvailabilityRecord>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Persists availability badge results (#251). The default is a no-op for
    /// fakes.
    fn save_availability(
        &self,
        _records: Vec<db::AvailabilityRecord>,
    ) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// One keyword-only release search on both lanes (#221 Q6); `refresh`
    /// bypasses the 60 s cache. The default is empty for fakes.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    fn search_releases(
        &self,
        _keyword: String,
        _refresh: bool,
    ) -> BoxFuture<Result<search::SearchOutcome, search::Error>> {
        Box::pin(async { Ok(search::SearchOutcome::default()) })
    }

    /// Every installed extension, enabled or not, by id (#304). The default
    /// is empty for fakes.
    fn installed_extensions(&self) -> BoxFuture<Result<Vec<ExtensionSummary>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Turns an installed extension on: its provider joins the next search.
    /// The default is a no-op for fakes.
    fn enable_extension(&self, _id: ExtensionId) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Turns an installed extension off, keeping its row: a call in flight
    /// is killed and its cached search rows are dropped, so the next search
    /// shows none of them. The default is a no-op for fakes.
    fn disable_extension(&self, _id: ExtensionId) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Deletes an installed extension's row, stopping it and dropping its
    /// cached search rows as [`Self::disable_extension`] does. The default is
    /// a no-op for fakes.
    fn uninstall_extension(&self, _id: ExtensionId) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// A torrent's file list, on demand for batch file pick (#221 Q7). The
    /// default is empty for fakes.
    fn torrent_files(
        &self,
        _torrent: search::Torrent,
    ) -> BoxFuture<Result<Vec<TorrentFileRow>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Auto-select profiles, the global default and per-media overrides.
    /// The default is the shipped defaults for fakes.
    fn auto_select_settings(&self) -> BoxFuture<Result<settings::AutoSelectSettings, EngineError>> {
        Box::pin(async { Ok(settings::AutoSelectSettings::default()) })
    }

    /// Sets or clears `media`'s per-media profile override. `media` is a raw
    /// [`MediaId`]; `profile` names a profile, `None` clears. The default is
    /// a no-op for fakes.
    fn set_media_profile(
        &self,
        _media: i64,
        _profile: Option<String>,
    ) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// The saved resume positions of `media`, by episode. The default is
    /// empty, for fakes.
    fn continuity(
        &self,
        _media: MediaId,
    ) -> BoxFuture<Result<Vec<(u32, Continuity)>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// A fresh Torrent List snapshot, newest first. The default is empty for
    /// fakes that do not exercise the Torrent screen.
    fn torrent_rows(&self) -> BoxFuture<Result<Vec<TorrentListRow>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Pauses a torrent, without dropping its files. The default is a no-op.
    fn torrent_pause(&self, _info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Resumes a paused torrent. The default is a no-op.
    fn torrent_resume(&self, _info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Match review (#223, #209 Q2-Q3, ADR-0009): parse `titles` through
    /// `library::matcher` (0.5, strictly above) against active-platform
    /// entries first, then AniList catalog results. Never auto-commits.
    /// The default is `MatchReview::Failed` for fakes.
    fn incoming_match(&self, _titles: Vec<String>) -> BoxFuture<Result<MatchReview, EngineError>> {
        Box::pin(async { Ok(MatchReview::Failed) })
    }

    /// Manual media pick (#209 Q3 `failed`): active-platform entries first,
    /// then AniList catalog results for `query`, as match candidates.
    /// Default empty for fakes.
    fn incoming_search_media(
        &self,
        _query: String,
    ) -> BoxFuture<Result<Vec<MatchCandidate>, EngineError>> {
        Box::pin(async { Ok(Vec::new()) })
    }

    /// Adds an incoming torrent to the session for metadata first (#223,
    /// #209 Q4), origin `TorrentOrigin::Incoming`. Default is a no-op error
    /// for fakes.
    fn incoming_add(
        &self,
        _source: IncomingSource,
    ) -> BoxFuture<Result<IncomingAdded, EngineError>> {
        Box::pin(async { Err(EngineError::Incoming("not available".to_owned())) })
    }

    /// Cancels an incoming torrent: drop + delete partials (#209 Q4, #180).
    /// The default is a no-op for fakes.
    fn incoming_cancel(&self, _info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Commits a confirmed match review: registers origin + (media, episode)
    /// on the Torrent List. Never auto-committed. Default is a no-op.
    fn incoming_commit(
        &self,
        _info_hash: InfoHash,
        _link: Option<TorrentLink>,
    ) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Selects the picked `file_index` for an incoming torrent and resumes
    /// it (#181 file pick, #223). Default is a no-op for fakes.
    fn incoming_select_file(
        &self,
        _info_hash: InfoHash,
        _file_index: usize,
    ) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
    }

    /// Removes a torrent and its cached files, ending a streaming playback
    /// session first. The default is a no-op.
    fn torrent_remove(&self, _info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        Box::pin(async { Ok(()) })
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
    /// The AniList catalog, for match-review candidate search (#223).
    anilist: AniList,
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
    /// The torrent session and relay, started by the first stream (#217).
    torrent: Arc<TorrentStreams>,
    /// Auto-select over the indexers and the torrent session (#218); rebuilt
    /// when the Network tab changes an indexer base URL.
    auto_select: Arc<RwLock<AutoSelector>>,
    /// The installed torrent providers' search lanes (#304).
    extensions: Arc<ExtensionLanes>,
}

/// Auto-select over both indexers (with `network`'s base URLs) and the
/// extension lanes, the session's file lists and `AniZip` eids.
fn auto_selector(
    client: &Client,
    poster_cache: &PosterCache,
    network: &settings::NetworkSettings,
    torrent: &Arc<TorrentStreams>,
    extensions: &Arc<ExtensionLanes>,
) -> AutoSelector {
    let transport = search::NetTransport::new(client.clone(), network::search_config(network));
    let lanes: Arc<dyn search::ProviderSet> = Arc::<ExtensionLanes>::clone(extensions);
    let searcher = search::Searcher::new(transport).with_providers(lanes);
    AutoSelector::new(
        Arc::new(Arc::new(searcher)),
        Arc::new(SessionFiles::new(Arc::clone(torrent))),
        Arc::new(AniZipEpisodeIds::new(AniZip::new(
            client.clone(),
            poster_cache.clone(),
        ))),
    )
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
    #[expect(
        clippy::too_many_lines,
        reason = "the composition root builds every service in dependency order"
    )]
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
        // One global outbound HTTP proxy for all app requests (ADR-0010).
        let builder = match settings.network.http_proxy.as_deref() {
            Some(proxy) if !proxy.trim().is_empty() => builder.proxy(proxy)?,
            _ => builder,
        };
        let client = builder.build()?;
        let poster_cache = PosterCache::new(paths.cache_dir())?;
        let anilist = AniList::new(client.clone());
        let catalog = CatalogCache::new(anilist.clone(), poster_cache.clone());
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
        let mut torrent_options = network::torrent_options(&settings.network);
        torrent_options.disable_upload = settings.torrent.disable_upload;
        let torrent = Arc::new(TorrentStreams::new(
            settings.torrent_folder(paths),
            torrent::PeerDiscovery::Public,
            torrent_options,
        ));
        // Enabled Luau providers from `autumn.db` become search lanes; with
        // none installed, search runs the built-in indexers only.
        let extensions = Arc::new(runtime.block_on(ExtensionLanes::load(&db)));
        let auto_select = Arc::new(RwLock::new(auto_selector(
            &client,
            &poster_cache,
            &settings.network,
            &torrent,
            &extensions,
        )));
        let playback = Arc::new(
            PlaybackManager::new(
                db.clone(),
                catalog.clone(),
                Arc::new(LocalProgressSync::new(db.clone())),
                events.clone(),
                &settings.playback,
                paths.cache_dir().to_path_buf(),
            )
            .with_torrent(Arc::clone(&torrent))
            .with_preload(
                Arc::clone(&auto_select),
                Arc::new(AniZipAiring::new(AniZip::new(
                    client.clone(),
                    poster_cache.clone(),
                ))),
                settings.auto_select.clone(),
            ),
        );
        // The Torrent List is pulled from librqbit, never polled by render().
        // One shared 4 Hz sampler feeds the store; it emits only on change so
        // an idle session produces no redraws (ARCHITECTURE §4, #207 Q5).
        let sampler_torrent = Arc::clone(&torrent);
        runtime.handle().spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_millis(250));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut last: Vec<TorrentListRow> = Vec::new();
            loop {
                tick.tick().await;
                let rows = sampler_torrent.rows().await.unwrap_or_default();
                if rows != last {
                    last.clone_from(&rows);
                    let _ = events.send(AppEvent::TorrentList(rows));
                }
            }
        });
        Ok(Self {
            db,
            client,
            poster_cache,
            catalog,
            anilist,
            platform,
            settings,
            library_settings,
            library,
            handle: runtime.handle().clone(),
            events: events_slot,
            scanner,
            watcher: Arc::new(Mutex::new(None)),
            playback,
            torrent,
            auto_select,
            extensions,
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

    /// Live Network reload (ADR-0010 §6). Rebuilds `net`'s shared client
    /// proxy for new requests (in-flight requests finish on the old one) and
    /// returns the [`network::TorrentSettingsChange`] diff-and-restart
    /// decision. [`Self::reload_network`] also acts on it.
    ///
    /// # Errors
    ///
    /// [`EngineError::Client`] if the proxy URL is invalid or the client
    /// cannot be rebuilt.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "one flag for whether a stream is currently playing"
    )]
    pub fn apply_network_settings(
        &self,
        old: &settings::NetworkSettings,
        new: &settings::NetworkSettings,
        stream_playing: bool,
    ) -> Result<network::TorrentSettingsChange, EngineError> {
        let proxy = new
            .http_proxy
            .as_deref()
            .map(str::trim)
            .filter(|proxy| !proxy.is_empty());
        self.client.set_proxy(proxy)?;
        Ok(network::torrent_settings_change(old, new, stream_playing))
    }

    /// Applies a Network save: the HTTP proxy and indexer base URLs for the
    /// next request, rate limits live, and a torrent session restart now or
    /// after the last stream ends ("Applies after playback stops").
    ///
    /// # Errors
    ///
    /// As [`Self::apply_network_settings`].
    pub async fn reload_network(
        &self,
        old: &settings::NetworkSettings,
        new: &settings::NetworkSettings,
    ) -> Result<network::TorrentSettingsChange, EngineError> {
        let change = self.apply_network_settings(old, new, self.torrent.is_streaming())?;
        let selector = auto_selector(
            &self.client,
            &self.poster_cache,
            new,
            &self.torrent,
            &self.extensions,
        );
        if let Ok(mut current) = self.auto_select.write() {
            *current = selector;
        }
        self.torrent
            .apply_network(&change, network::torrent_options(new))
            .await;
        Ok(change)
    }

    /// The torrent session and relay behind torrent streams.
    #[must_use]
    pub fn torrent_streams(&self) -> &Arc<TorrentStreams> {
        &self.torrent
    }

    /// Applies a lifecycle `change` to extension `id`, then drops its cached
    /// search rows (ADR 0011) once the new lanes no longer run its old actor.
    fn change_extension(
        &self,
        id: ExtensionId,
        change: Change,
    ) -> BoxFuture<Result<(), EngineError>> {
        let db = self.db.clone();
        let extensions = Arc::clone(&self.extensions);
        let auto_select = Arc::clone(&self.auto_select);
        Box::pin(async move {
            extensions.apply(&db, &id, change).await?;
            let selector = auto_select.read().ok().map(|selector| selector.clone());
            if let Some(selector) = selector {
                selector.purge_provider(&id);
            }
            Ok(())
        })
    }

    /// The auto-select pipeline with the current Network settings.
    #[must_use]
    pub fn auto_selector(&self) -> Option<AutoSelector> {
        self.auto_select
            .read()
            .ok()
            .map(|selector| selector.clone())
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
        let torrent = Arc::clone(&self.torrent);
        let db = self.db.clone();
        let request = request.clone();
        Box::pin(async move {
            if let StreamSource::Torrent { info_hash, .. } = request.source {
                // The row is a stream and, when the media is known, links to
                // its Entry and episode (#207 Q1, Q8).
                let anilist_id = db
                    .media()
                    .external_ids(request.media)
                    .await
                    .unwrap_or_default()
                    .into_iter()
                    .find_map(|id| match id {
                        ExternalId::AniList(id) => Some(id),
                        _ => None,
                    });
                torrent.register(
                    info_hash,
                    TorrentOrigin::Stream,
                    Some(TorrentLink::new(request.media, request.episode, anilist_id)),
                );
            }
            playback.play(request).await.map_err(EngineError::from)
        })
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

    fn play_next_now(&self) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            if playback.play_next_now() {
                Ok(())
            } else {
                Err(EngineError::NoSession)
            }
        })
    }

    fn cancel_auto_next(&self) -> BoxFuture<Result<(), EngineError>> {
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            if playback.cancel_auto_next() {
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

    fn resume_points(&self) -> BoxFuture<Result<Vec<ResumePoint>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move { db.continuity().all().await.map_err(EngineError::from) })
    }

    fn airing_schedule(&self) -> BoxFuture<Result<Load<Vec<AiringEpisode>>, EngineError>> {
        let catalog = self.catalog.clone();
        Box::pin(async move { catalog.airing_schedule().await.map_err(EngineError::from) })
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

    fn episode_metadata(
        &self,
        anilist_id: u32,
    ) -> BoxFuture<Result<Option<EpisodeMetadata>, EngineError>> {
        let anizip = AniZip::new(self.client.clone(), self.poster_cache.clone());
        Box::pin(async move {
            // Airing's shorter TTL keeps a new episode's title from lagging a
            // week; a finished show costs one cached request a day.
            anizip
                .media(anilist_id, Aging::Airing)
                .await
                .map_err(EngineError::from)
        })
    }

    fn auto_select(
        &self,
        request: AutoSelectRequest,
        profile: settings::AutoSelectProfile,
        progress: Box<dyn FnMut(AutoSelectProgress) + Send>,
    ) -> BoxFuture<Result<AutoSelection, AutoSelectError>> {
        let selector = self.auto_selector();
        Box::pin(async move {
            match selector {
                Some(selector) => {
                    selector
                        .select_with_progress(request, &profile, progress)
                        .await
                }
                None => Err(AutoSelectError::IndexerError(search::Error::Transport {
                    indexer: search::Indexer::Nyaa,
                    message: "auto-select is unavailable".to_owned(),
                })),
            }
        })
    }

    fn search_torrents(
        &self,
        request: search::SearchRequest,
        lanes: search::LaneFilter,
    ) -> BoxFuture<Result<search::SearchOutcome, search::Error>> {
        let selector = self.auto_selector();
        Box::pin(async move {
            match selector {
                Some(selector) => selector.search(request, lanes).await,
                None => Err(search::Error::AllLanesFailed),
            }
        })
    }

    fn availability(
        &self,
        media: i64,
    ) -> BoxFuture<Result<Vec<db::AvailabilityRecord>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.availability()
                .for_media(media)
                .await
                .map_err(EngineError::from)
        })
    }

    fn save_availability(
        &self,
        records: Vec<db::AvailabilityRecord>,
    ) -> BoxFuture<Result<(), EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.availability()
                .save(&records)
                .await
                .map_err(EngineError::from)
        })
    }

    fn search_releases(
        &self,
        keyword: String,
        refresh: bool,
    ) -> BoxFuture<Result<search::SearchOutcome, search::Error>> {
        let selector = self.auto_selector();
        Box::pin(async move {
            match selector {
                Some(selector) => selector.search_keyword(&keyword, refresh).await,
                None => Err(search::Error::AllLanesFailed),
            }
        })
    }

    fn installed_extensions(&self) -> BoxFuture<Result<Vec<ExtensionSummary>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move { Ok(extension_lanes::installed(&db).await?) })
    }

    fn enable_extension(&self, id: ExtensionId) -> BoxFuture<Result<(), EngineError>> {
        self.change_extension(id, Change::Enable)
    }

    fn disable_extension(&self, id: ExtensionId) -> BoxFuture<Result<(), EngineError>> {
        self.change_extension(id, Change::Disable)
    }

    fn uninstall_extension(&self, id: ExtensionId) -> BoxFuture<Result<(), EngineError>> {
        self.change_extension(id, Change::Uninstall)
    }

    fn torrent_files(
        &self,
        torrent: search::Torrent,
    ) -> BoxFuture<Result<Vec<TorrentFileRow>, EngineError>> {
        let selector = self.auto_selector();
        Box::pin(async move {
            let selector = selector.ok_or_else(|| {
                EngineError::ActivePlatform("torrent search is unavailable".to_owned())
            })?;
            let files = selector
                .list_files(&torrent)
                .await
                .map_err(|error| EngineError::ActivePlatform(error.to_string()))?;
            Ok(files
                .into_iter()
                .map(|file| TorrentFileRow::new(file.index, file.path, file.len, false))
                .collect())
        })
    }

    fn auto_select_settings(&self) -> BoxFuture<Result<settings::AutoSelectSettings, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            let rows = db.settings().rows().await?;
            let (settings, _) = settings::Settings::from_rows(rows);
            Ok(settings.auto_select)
        })
    }

    fn set_media_profile(
        &self,
        media: i64,
        profile: Option<String>,
    ) -> BoxFuture<Result<(), EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            let rows = db.settings().rows().await?;
            let (mut settings, _) = settings::Settings::from_rows(rows);
            match profile {
                Some(name) => {
                    settings.auto_select.media_profiles.insert(media, name);
                }
                None => {
                    settings.auto_select.media_profiles.remove(&media);
                }
            }
            let row = settings
                .to_rows()?
                .into_iter()
                .filter(|(tab, _)| tab == "auto_select")
                .collect::<Vec<_>>();
            db.settings().save(&row).await?;
            Ok(())
        })
    }

    fn continuity(&self, media: MediaId) -> BoxFuture<Result<Vec<(u32, Continuity)>, EngineError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.continuity()
                .for_media(media)
                .await
                .map_err(EngineError::from)
        })
    }

    fn torrent_rows(&self) -> BoxFuture<Result<Vec<TorrentListRow>, EngineError>> {
        let torrent = Arc::clone(&self.torrent);
        Box::pin(async move { torrent.rows().await.map_err(EngineError::from) })
    }

    fn torrent_pause(&self, info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        let torrent = Arc::clone(&self.torrent);
        Box::pin(async move {
            let session = torrent.session().await?;
            session.pause(info_hash).await?;
            Ok(())
        })
    }

    fn torrent_resume(&self, info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        let torrent = Arc::clone(&self.torrent);
        Box::pin(async move {
            let session = torrent.session().await?;
            session.resume(info_hash).await?;
            Ok(())
        })
    }

    fn incoming_match(&self, titles: Vec<String>) -> BoxFuture<Result<MatchReview, EngineError>> {
        let engine = self.clone();
        Box::pin(async move { engine.match_review_load(titles).await })
    }

    fn incoming_search_media(
        &self,
        query: String,
    ) -> BoxFuture<Result<Vec<MatchCandidate>, EngineError>> {
        let engine = self.clone();
        Box::pin(async move { engine.search_media_candidates(query).await })
    }

    fn incoming_add(
        &self,
        source: IncomingSource,
    ) -> BoxFuture<Result<IncomingAdded, EngineError>> {
        let streams = Arc::clone(&self.torrent);
        Box::pin(async move {
            add_incoming(&streams, &source)
                .await
                .map_err(|error| EngineError::Incoming(error.to_string()))
        })
    }

    fn incoming_cancel(&self, info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        let streams = Arc::clone(&self.torrent);
        Box::pin(async move {
            cancel_incoming(&streams, info_hash).await;
            Ok(())
        })
    }

    fn incoming_commit(
        &self,
        info_hash: InfoHash,
        link: Option<TorrentLink>,
    ) -> BoxFuture<Result<(), EngineError>> {
        let streams = Arc::clone(&self.torrent);
        Box::pin(async move {
            commit_incoming(&streams, info_hash, link);
            Ok(())
        })
    }

    fn incoming_select_file(
        &self,
        info_hash: InfoHash,
        file_index: usize,
    ) -> BoxFuture<Result<(), EngineError>> {
        let streams = Arc::clone(&self.torrent);
        Box::pin(async move {
            let session = streams
                .session()
                .await
                .map_err(|error| EngineError::Incoming(error.to_string()))?;
            session
                .update_only_files(info_hash, &[file_index])
                .await
                .map_err(|error| EngineError::Incoming(error.to_string()))?;
            session
                .resume(info_hash)
                .await
                .map_err(|error| EngineError::Incoming(error.to_string()))?;
            Ok(())
        })
    }

    fn torrent_remove(&self, info_hash: InfoHash) -> BoxFuture<Result<(), EngineError>> {
        let torrent = Arc::clone(&self.torrent);
        let playback = Arc::clone(&self.playback);
        Box::pin(async move {
            let streaming = torrent.is_streaming_torrent(info_hash);
            torrent
                .remove(info_hash, async {
                    if streaming {
                        // stops the session: revoke the relay token, drop
                        // readers, and only then delete the cached files.
                        playback.stop().await;
                    }
                })
                .await
                .map_err(EngineError::from)
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

    /// Match review candidate loading (#223): active-platform entries
    /// first, then AniList catalog results; `match_review` scores them.
    async fn match_review_load(&self, titles: Vec<String>) -> Result<MatchReview, EngineError> {
        let mut candidates = self.incoming_candidates().await?;
        // AniList catalog results after the active platform's entries.
        if let Some(first) = titles.first() {
            let filters = metadata::SearchFilters {
                search: Some(first.clone()),
                kind: Some(MediaKind::Anime),
                ..metadata::SearchFilters::default()
            };
            if let Ok(page) = self
                .anilist
                .search(&filters, metadata::PageRequest::new(1, 10))
                .await
            {
                let seen: HashSet<String> = candidates
                    .iter()
                    .flat_map(|c| std::iter::once(c.title.clone()).chain(c.aliases.iter().cloned()))
                    .map(|t| t.to_lowercase())
                    .collect();
                for media in page.media {
                    if media.anilist_id == 0 {
                        continue;
                    }
                    let Some(title) = media
                        .title
                        .romaji
                        .clone()
                        .or_else(|| media.title.english.clone())
                        .or_else(|| media.title.native.clone())
                    else {
                        continue;
                    };
                    if seen.contains(&title.to_lowercase()) {
                        continue;
                    }
                    let mut aliases = Vec::new();
                    for alias in [
                        media.title.english.as_ref(),
                        media.title.native.as_ref(),
                        media.title.romaji.as_ref(),
                    ]
                    .into_iter()
                    .flatten()
                    {
                        if *alias != title {
                            aliases.push(alias.clone());
                        }
                    }
                    let mut external = vec![ExternalId::AniList(media.anilist_id)];
                    if let Some(mal) = media.mal_id {
                        external.push(ExternalId::Mal(mal));
                    }
                    candidates.push(MatchCandidate {
                        media: None,
                        title,
                        aliases,
                        external_ids: external,
                    });
                }
            }
        }
        Ok(match_review(&titles, &candidates))
    }

    /// Titled candidates from the active platform's anime list entries,
    /// with known media ids, ordered first.
    async fn incoming_candidates(&self) -> Result<Vec<MatchCandidate>, EngineError> {
        let mut out = Vec::new();
        for entry in self.entries(MediaKind::Anime).await? {
            let ids = self.db.media().external_ids(entry.media).await?;
            let anilist = ids.iter().find_map(|id| match id {
                ExternalId::AniList(id) => Some(*id),
                _ => None,
            });
            let summary = match anilist {
                Some(id) => self.catalog.cached_media(id).await.map(|c| c.value.summary),
                None => None,
            };
            let Some(summary) = summary else {
                continue;
            };
            let Some(title) = summary
                .title
                .romaji
                .clone()
                .or_else(|| summary.title.english.clone())
                .or_else(|| summary.title.native.clone())
            else {
                continue;
            };
            let mut aliases = Vec::new();
            for alias in [
                summary.title.english.as_ref(),
                summary.title.native.as_ref(),
                summary.title.romaji.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                if *alias != title {
                    aliases.push(alias.clone());
                }
            }
            out.push(MatchCandidate {
                media: Some(entry.media),
                title,
                aliases,
                external_ids: ids,
            });
        }
        Ok(out)
    }

    /// Manual media pick candidates: active platform first, then AniList.
    async fn search_media_candidates(
        &self,
        query: String,
    ) -> Result<Vec<MatchCandidate>, EngineError> {
        let mut candidates = self.incoming_candidates().await?;
        let filters = metadata::SearchFilters {
            search: Some(query),
            kind: Some(MediaKind::Anime),
            ..metadata::SearchFilters::default()
        };
        if let Ok(page) = self
            .anilist
            .search(&filters, metadata::PageRequest::new(1, 10))
            .await
        {
            for media in page.media {
                if media.anilist_id == 0 {
                    continue;
                }
                let Some(title) = media
                    .title
                    .romaji
                    .clone()
                    .or_else(|| media.title.english.clone())
                    .or_else(|| media.title.native.clone())
                else {
                    continue;
                };
                candidates.push(MatchCandidate {
                    media: None,
                    title,
                    aliases: Vec::new(),
                    external_ids: vec![ExternalId::AniList(media.anilist_id)],
                });
            }
        }
        Ok(candidates)
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
