//! Engine fakes for store and screen tests (#58, #61).
//!
//! Screens reach the engine only through ui-state stores, so their test
//! fakes live here with the stores rather than in `ui-screens`.

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
};

use db::{Continuity, LocalFile, ScanSummary};
use domain::{ExternalId, ListEntry, MediaId, MediaKind, PlaybackControl};
use engine::{BoxFuture, EngineError, EngineServices, LibraryMediaInfo, ListedEntry, PlayRequest};
use metadata::{HomeLoad, Load, MediaDetail};

/// An engine service whose calls never resolve, for tests that only need a
/// store to construct without loading data.
#[derive(Debug)]
pub struct PendingServices;

impl Default for PendingServices {
    fn default() -> Self {
        Self
    }
}

/// Returns a pending engine fake behind the service trait.
#[must_use]
pub fn pending_services() -> Arc<dyn EngineServices> {
    Arc::new(PendingServices)
}

/// An engine fake that serves one Entry detail and records its saves. The
/// Home calls stay pending unless a test configures them.
#[derive(Debug, Clone)]
pub struct CannedServices {
    detail: MediaDetail,
    saved: Arc<Mutex<Vec<ListEntry>>>,
    /// Whether the Home catalog load fails; otherwise it stays pending.
    home_fails: bool,
    /// The Home collection, when a test serves one.
    collection: Option<Vec<ListedEntry>>,
}

impl CannedServices {
    /// Serves `detail` from the Entry store.
    #[must_use]
    pub fn new(detail: MediaDetail) -> Self {
        Self {
            detail,
            saved: Arc::new(Mutex::new(Vec::new())),
            home_fails: false,
            collection: None,
        }
    }

    /// Fails the Home catalog load, so its three sections show errors.
    #[must_use]
    pub fn home_fails(mut self) -> Self {
        self.home_fails = true;
        self
    }

    /// Serves `entries` as the Home collection, and no missed sequels.
    #[must_use]
    pub fn collection(mut self, entries: Vec<ListedEntry>) -> Self {
        self.collection = Some(entries);
        self
    }

    /// The entries handed to `save_entry`, oldest first.
    #[must_use]
    pub fn saved(&self) -> Vec<ListEntry> {
        self.saved
            .lock()
            .map_or_else(|_| Vec::new(), |saved| saved.clone())
    }
}

/// Returns a canned engine fake behind the service trait.
#[must_use]
pub fn canned_services(detail: MediaDetail) -> Arc<dyn EngineServices> {
    Arc::new(CannedServices::new(detail))
}

impl EngineServices for CannedServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        let detail = self.detail.clone();
        Box::pin(async move {
            Ok(Load {
                cached: metadata::Cached {
                    value: detail,
                    updated_at: std::time::SystemTime::now(),
                    offline: false,
                },
                refresh: None,
            })
        })
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        Box::pin(async move { Ok(Some(MediaId::new(1))) })
    }

    fn local_entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        Box::pin(async move { Ok(None) })
    }

    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        let entry = entry.clone();
        let saved = Arc::clone(&self.saved);
        Box::pin(async move {
            if let Ok(mut saved) = saved.lock() {
                saved.push(entry.clone());
            }
            Ok(entry)
        })
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(async move { Ok(MediaId::new(1)) })
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        if self.home_fails {
            Box::pin(async { Err(EngineError::Metadata(metadata::Error::NotFound)) })
        } else {
            Box::pin(std::future::pending())
        }
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        match self.collection.clone() {
            Some(entries) => Box::pin(async move { Ok(entries) }),
            None => Box::pin(std::future::pending()),
        }
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        if self.collection.is_some() {
            Box::pin(async { Ok(Vec::new()) })
        } else {
            Box::pin(std::future::pending())
        }
    }
}

impl EngineServices for PendingServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn save_entry(&self, _entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        Box::pin(std::future::pending())
    }
}

/// A library command recorded by [`LibraryServices`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LibraryCommand {
    /// A manual match of one file to an AniList media.
    Match {
        /// The file to match.
        path: PathBuf,
        /// The AniList media chosen by the user.
        anilist_id: u32,
        /// The episode, when the dialog supplied one.
        episode: Option<u32>,
    },
    /// A file marked as ignored.
    Ignore(PathBuf),
}

/// An engine fake that records scans and serves a canned library.
#[derive(Debug, Clone, Default)]
pub struct LibraryServices {
    scans: Arc<AtomicUsize>,
    summary: Arc<Mutex<Option<ScanSummary>>>,
    summaries: Arc<Mutex<Vec<ScanSummary>>>,
    media_info: Arc<Mutex<Vec<LibraryMediaInfo>>>,
    files: Arc<Mutex<Vec<LocalFile>>>,
    commands: Arc<Mutex<Vec<LibraryCommand>>>,
    failing: Arc<AtomicBool>,
}

impl LibraryServices {
    /// Serves `summary` and `files` from the library calls.
    #[must_use]
    pub fn new(summary: ScanSummary, files: Vec<LocalFile>) -> Self {
        Self {
            scans: Arc::new(AtomicUsize::new(0)),
            summary: Arc::new(Mutex::new(Some(summary))),
            summaries: Arc::new(Mutex::new(Vec::new())),
            media_info: Arc::new(Mutex::new(Vec::new())),
            files: Arc::new(Mutex::new(files)),
            commands: Arc::new(Mutex::new(Vec::new())),
            failing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Serves `summaries` as the stored scan log.
    #[must_use]
    pub fn with_summaries(mut self, summaries: Vec<ScanSummary>) -> Self {
        self.summaries = Arc::new(Mutex::new(summaries));
        self
    }

    /// Replaces the canned log, for tests where a newer scan is inserted.
    pub fn set_summaries(&self, summaries: Vec<ScanSummary>) {
        if let Ok(mut slot) = self.summaries.lock() {
            *slot = summaries;
        }
    }

    /// Serves `media_info` for Entry links in the summaries screen.
    #[must_use]
    pub fn with_media(mut self, media_info: Vec<LibraryMediaInfo>) -> Self {
        self.media_info = Arc::new(Mutex::new(media_info));
        self
    }

    /// How many scans were requested.
    #[must_use]
    pub fn scan_count(&self) -> usize {
        self.scans.load(Ordering::SeqCst)
    }

    /// Makes the log load and the match and ignore commands fail from now on.
    pub fn fail_calls(&self) {
        self.failing.store(true, Ordering::SeqCst);
    }

    /// The result of a call, failed after [`Self::fail_calls`].
    fn outcome<T>(failing: &AtomicBool, value: T) -> Result<T, EngineError> {
        if failing.load(Ordering::SeqCst) {
            Err(EngineError::ActivePlatform(
                "the fake engine failed".to_owned(),
            ))
        } else {
            Ok(value)
        }
    }

    /// The match and ignore commands that reached the engine, oldest first.
    #[must_use]
    pub fn commands(&self) -> Vec<LibraryCommand> {
        self.commands
            .lock()
            .map_or_else(|_| Vec::new(), |commands| commands.clone())
    }
}

impl EngineServices for LibraryServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn save_entry(&self, _entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn start_scan(&self) -> BoxFuture<Result<(), EngineError>> {
        self.scans.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(()) })
    }

    fn latest_scan_summary(&self) -> BoxFuture<Result<Option<ScanSummary>, EngineError>> {
        let summary = self
            .summary
            .lock()
            .map_or_else(|_| None, |summary| summary.clone());
        Box::pin(async move { Ok(summary) })
    }

    fn library_media(&self) -> BoxFuture<Result<Vec<MediaId>, EngineError>> {
        let media: Vec<MediaId> = self.files.lock().map_or_else(
            |_| Vec::new(),
            |files| {
                let mut ids: Vec<MediaId> = files.iter().filter_map(|file| file.media_id).collect();
                ids.sort_by_key(|id| id.get());
                ids.dedup();
                ids
            },
        );
        Box::pin(async move { Ok(media) })
    }

    fn all_library_files(&self) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        let files = self
            .files
            .lock()
            .map_or_else(|_| Vec::new(), |files| files.clone());
        Box::pin(async move { Ok(files) })
    }

    fn library_files(&self, media: MediaId) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        let files: Vec<LocalFile> = self.files.lock().map_or_else(
            |_| Vec::new(),
            |files| {
                files
                    .iter()
                    .filter(|file| file.media_id == Some(media))
                    .cloned()
                    .collect()
            },
        );
        Box::pin(async move { Ok(files) })
    }

    fn summaries(&self) -> BoxFuture<Result<Vec<ScanSummary>, EngineError>> {
        let summaries = self
            .summaries
            .lock()
            .map_or_else(|_| Vec::new(), |summaries| summaries.clone());
        let failing = Arc::clone(&self.failing);
        Box::pin(async move { Self::outcome(&failing, summaries) })
    }

    fn library_media_info(&self) -> BoxFuture<Result<Vec<LibraryMediaInfo>, EngineError>> {
        let media = self
            .media_info
            .lock()
            .map_or_else(|_| Vec::new(), |media| media.clone());
        Box::pin(async move { Ok(media) })
    }

    fn manual_match(
        &self,
        paths: &[PathBuf],
        anilist_id: u32,
        episode: Option<u32>,
    ) -> BoxFuture<Result<usize, EngineError>> {
        // Recorded when the future runs, so a dropped command never counts.
        let (commands, failing) = (Arc::clone(&self.commands), Arc::clone(&self.failing));
        let path = paths.first().cloned();
        Box::pin(async move {
            if let (Ok(mut commands), Some(path)) = (commands.lock(), path) {
                commands.push(LibraryCommand::Match {
                    path,
                    anilist_id,
                    episode,
                });
            }
            Self::outcome(&failing, 1)
        })
    }

    fn ignore_file(&self, path: &std::path::Path) -> BoxFuture<Result<usize, EngineError>> {
        let (commands, failing) = (Arc::clone(&self.commands), Arc::clone(&self.failing));
        let path = path.to_path_buf();
        Box::pin(async move {
            if let Ok(mut commands) = commands.lock() {
                commands.push(LibraryCommand::Ignore(path));
            }
            Self::outcome(&failing, 1)
        })
    }
}

/// One playback command sent through a store, recorded for command tests.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlaybackCall {
    /// A play request.
    Play(PlayRequest),
    /// A stop request.
    Stop,
    /// A retry of a failed progress sync.
    Retry(MediaId, u32),
    /// A play/pause command.
    Pause(PlaybackControl),
    /// An absolute seek, in seconds.
    Seek(f64),
    /// A request to skip the current chapter.
    SkipChapter,
}

/// An engine fake that records playback commands; other calls stay pending.
#[derive(Debug, Default, Clone)]
pub struct PlaybackServices {
    calls: Arc<Mutex<Vec<PlaybackCall>>>,
}

impl PlaybackServices {
    /// The commands received, oldest first.
    #[must_use]
    pub fn calls(&self) -> Vec<PlaybackCall> {
        self.calls
            .lock()
            .map_or_else(|_| Vec::new(), |calls| calls.clone())
    }
}

impl EngineServices for PlaybackServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn save_entry(&self, _entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn play(&self, request: &PlayRequest) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Play(request.clone()));
        }
        Box::pin(async { Ok(()) })
    }

    fn stop(&self) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Stop);
        }
        Box::pin(async { Ok(()) })
    }

    fn retry_sync(&self, media: MediaId, episode: u32) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Retry(media, episode));
        }
        Box::pin(async { Ok(()) })
    }

    fn set_playback_state(&self, state: PlaybackControl) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Pause(state));
        }
        Box::pin(async { Ok(()) })
    }

    fn seek(&self, seconds: f64) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::Seek(seconds));
        }
        Box::pin(async { Ok(()) })
    }

    fn skip_chapter(&self) -> BoxFuture<Result<(), EngineError>> {
        if let Ok(mut calls) = self.calls.lock() {
            calls.push(PlaybackCall::SkipChapter);
        }
        Box::pin(async { Ok(()) })
    }
}

/// A library row for `path`, matched to `media` as `kind` episode `episode`.
#[must_use]
pub fn library_file(path: &str, media: MediaId, kind: &str, episode: Option<u32>) -> LocalFile {
    LocalFile {
        path: path.into(),
        size: 0,
        modified: 0,
        title: None,
        group: None,
        season: None,
        episode,
        kind: kind.to_owned(),
        media_id: Some(media),
        confidence: Some(0.9),
        locked: false,
        ignored: false,
        filler: false,
        shelved: false,
    }
}

/// An engine fake for the Entry Library tab: one media, its files, `AniZip`
/// episode data, a list entry's progress and resume positions.
#[derive(Debug, Clone)]
pub struct EntryLibraryServices {
    media: MediaId,
    files: Arc<Mutex<Vec<LocalFile>>>,
    metadata: Option<metadata::Media>,
    progress: Arc<Mutex<Option<u32>>>,
    continuity: Vec<(u32, Continuity)>,
    commands: Arc<Mutex<Vec<EntryLibraryCommand>>>,
}

/// A bulk action recorded by [`EntryLibraryServices`].
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryLibraryCommand {
    /// A file's lock set or cleared.
    Lock(PathBuf, db::FileFlag),
    /// Files matched to another AniList media.
    Match(Vec<PathBuf>, u32),
    /// Files moved to the trash, in one call.
    Trash(Vec<PathBuf>),
}

impl EntryLibraryServices {
    /// The bulk actions that reached the engine, oldest first.
    #[must_use]
    pub fn commands(&self) -> Vec<EntryLibraryCommand> {
        self.commands
            .lock()
            .map_or_else(|_| Vec::new(), |commands| commands.clone())
    }

    fn record(&self, command: EntryLibraryCommand) -> BoxFuture<Result<usize, EngineError>> {
        let commands = Arc::clone(&self.commands);
        Box::pin(async move {
            if let Ok(mut commands) = commands.lock() {
                commands.push(command);
            }
            Ok(1)
        })
    }

    /// Serves `files` as the local files of `media`, the media behind every
    /// AniList id.
    #[must_use]
    pub fn new(media: MediaId, files: Vec<LocalFile>) -> Self {
        Self {
            media,
            files: Arc::new(Mutex::new(files)),
            metadata: None,
            progress: Arc::new(Mutex::new(None)),
            continuity: Vec::new(),
            commands: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Serves `metadata` as the media's `AniZip` episode data.
    #[must_use]
    pub fn with_metadata(mut self, metadata: metadata::Media) -> Self {
        self.metadata = Some(metadata);
        self
    }

    /// Serves a list entry at `progress` episodes watched.
    #[must_use]
    pub fn with_progress(self, progress: u32) -> Self {
        self.set_progress(progress);
        self
    }

    /// Changes the list entry's progress, as a playback sync would.
    pub fn set_progress(&self, progress: u32) {
        if let Ok(mut slot) = self.progress.lock() {
            *slot = Some(progress);
        }
    }

    /// Serves `continuity` as the saved resume positions.
    #[must_use]
    pub fn with_continuity(mut self, continuity: Vec<(u32, Continuity)>) -> Self {
        self.continuity = continuity;
        self
    }

    /// Replaces the served files, as a scan or a library action would.
    pub fn set_files(&self, files: Vec<LocalFile>) {
        if let Ok(mut slot) = self.files.lock() {
            *slot = files;
        }
    }
}

impl EngineServices for EntryLibraryServices {
    fn detail(&self, _anilist_id: u32) -> BoxFuture<Result<Load<MediaDetail>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn local_media(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<Option<MediaId>, EngineError>> {
        let media = self.media;
        Box::pin(async move { Ok(Some(media)) })
    }

    fn local_entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, EngineError>> {
        let progress = self.progress.lock().map_or(None, |slot| *slot);
        Box::pin(async move {
            Ok(progress.map(|progress| ListEntry {
                media,
                status: domain::ListStatus::Current,
                score: domain::Score::UNSCORED,
                progress,
                progress_volumes: 0,
                repeat: 0,
                started_at: domain::FuzzyDate::UNKNOWN,
                completed_at: domain::FuzzyDate::UNKNOWN,
                notes: String::new(),
                private: false,
                priority: 0,
                advanced_scores: domain::AdvancedScores::default(),
                custom_lists: Vec::new(),
                hidden_from_status_lists: false,
                created_at: std::time::SystemTime::UNIX_EPOCH,
                updated_at: std::time::SystemTime::UNIX_EPOCH,
            }))
        })
    }

    fn save_entry(&self, _entry: &ListEntry) -> BoxFuture<Result<ListEntry, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn mint(
        &self,
        _kind: MediaKind,
        _ids: &[ExternalId],
    ) -> BoxFuture<Result<MediaId, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn home(&self) -> BoxFuture<Result<Load<HomeLoad>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn collection(&self, _kind: MediaKind) -> BoxFuture<Result<Vec<ListedEntry>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn missed_sequels(
        &self,
        _kind: MediaKind,
    ) -> BoxFuture<Result<Vec<metadata::MediaSummary>, EngineError>> {
        Box::pin(std::future::pending())
    }

    fn library_files(&self, media: MediaId) -> BoxFuture<Result<Vec<LocalFile>, EngineError>> {
        let files: Vec<LocalFile> = self.files.lock().map_or_else(
            |_| Vec::new(),
            |files| {
                files
                    .iter()
                    .filter(|file| file.media_id == Some(media))
                    .cloned()
                    .collect()
            },
        );
        Box::pin(async move { Ok(files) })
    }

    fn episode_metadata(
        &self,
        _anilist_id: u32,
    ) -> BoxFuture<Result<Option<metadata::Media>, EngineError>> {
        let metadata = self.metadata.clone();
        Box::pin(async move { Ok(metadata) })
    }

    fn continuity(
        &self,
        _media: MediaId,
    ) -> BoxFuture<Result<Vec<(u32, Continuity)>, EngineError>> {
        let continuity = self.continuity.clone();
        Box::pin(async move { Ok(continuity) })
    }

    fn set_library_lock(
        &self,
        path: &std::path::Path,
        flag: db::FileFlag,
    ) -> BoxFuture<Result<usize, EngineError>> {
        self.record(EntryLibraryCommand::Lock(path.to_path_buf(), flag))
    }

    fn manual_match(
        &self,
        paths: &[PathBuf],
        anilist_id: u32,
        _episode: Option<u32>,
    ) -> BoxFuture<Result<usize, EngineError>> {
        self.record(EntryLibraryCommand::Match(paths.to_vec(), anilist_id))
    }

    fn trash_library_files(&self, paths: &[PathBuf]) -> BoxFuture<Result<usize, EngineError>> {
        self.record(EntryLibraryCommand::Trash(paths.to_vec()))
    }
}
