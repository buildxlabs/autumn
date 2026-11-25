//! The engine's library module: one scan at a time, candidates from the active
//! platform and the anime-offline-database index (ticket #127).
//!
//! The scan itself runs on Tokio through the `library` crate. This module only
//! gathers candidates, throttles progress to 4 Hz and publishes the result as
//! [`AppEvent`]s for the UI stores (ARCHITECTURE §4, ADR 0004).

use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use ::library::{Candidate, Library, LibraryError, ScanSummary};
use db::{Db, LocalFile};
use domain::{AppEvent, ExternalId, ListEntry, MediaId, MediaKind};
use flume::Sender;
use metadata::{
    AnimeFillerList, AnimeOfflineDatabase, AnimeOfflineIds, CatalogCache, FillerLookup,
    OfflineAnime,
};
use net::{Client, PosterCache};
use thiserror::Error;
use tokio::runtime::Handle;
use tracing::debug;

use crate::{BoxFuture, EngineError};

/// Progress events are coalesced to at most four per second.
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Whether a scan looks up filler lists (OD-3's Library setting).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FillerPolicy {
    /// Look up filler lists through `metadata`.
    Enabled,
    /// Leave episodes unmarked without a request.
    Disabled,
}

impl From<bool> for FillerPolicy {
    fn from(enabled: bool) -> Self {
        if enabled {
            Self::Enabled
        } else {
            Self::Disabled
        }
    }
}

/// A scan was refused because another one is still running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[non_exhaustive]
pub enum ScanError {
    /// Only one library scan may run at a time.
    #[error("a library scan is already running")]
    AlreadyScanning,
}

/// The active platform's anime list entries, as seen by the composition root.
///
/// The engine owns the Local platform by default; the binary replaces this
/// source with one over the active platform gateway after login (ADR 0004).
pub trait AnimeList: std::fmt::Debug + Send + Sync + 'static {
    /// The active platform's anime entries, whatever their list status.
    fn anime_entries(&self) -> BoxFuture<Result<Vec<ListEntry>, EngineError>>;
}

impl AnimeList for tracker::Local {
    fn anime_entries(&self) -> BoxFuture<Result<Vec<ListEntry>, EngineError>> {
        let local = self.clone();
        Box::pin(async move {
            use tracker::Platform;

            let collection = local.collection(MediaKind::Anime).await?;
            Ok(collection
                .lists
                .into_iter()
                .flat_map(|list| list.entries)
                .collect())
        })
    }
}

/// Supplies the titles a scan matches against, injected for tests.
pub trait CandidateSource: std::fmt::Debug + Send + Sync + 'static {
    /// Builds the candidate list for one scan.
    fn load(&self) -> BoxFuture<Result<Vec<Candidate>, EngineError>>;
}

/// Candidates from the active platform's list plus the offline database.
#[derive(Debug, Clone)]
pub struct PlatformCandidates {
    entries: Arc<dyn AnimeList>,
    db: Db,
    catalog: CatalogCache,
    client: Client,
    disk: PosterCache,
}

impl PlatformCandidates {
    /// Builds a candidate source over `entries`, the catalog cache and the
    /// cached anime-offline-database index.
    #[must_use]
    pub fn new(
        entries: Arc<dyn AnimeList>,
        db: Db,
        catalog: CatalogCache,
        client: Client,
        disk: PosterCache,
    ) -> Self {
        Self {
            entries,
            db,
            catalog,
            client,
            disk,
        }
    }
}

impl CandidateSource for PlatformCandidates {
    fn load(&self) -> BoxFuture<Result<Vec<Candidate>, EngineError>> {
        let (entries, db, catalog, client, disk) = (
            Arc::clone(&self.entries),
            self.db.clone(),
            self.catalog.clone(),
            self.client.clone(),
            self.disk.clone(),
        );
        Box::pin(async move {
            let index = AnimeOfflineDatabase::load(&client, &disk).await;
            let mut own = Vec::new();
            for entry in entries.anime_entries().await? {
                let ids = db.media().external_ids(entry.media).await?;
                let anilist = ids.iter().find_map(|id| match id {
                    ExternalId::AniList(id) => Some(*id),
                    _ => None,
                });
                let Some(anilist) = anilist else {
                    continue;
                };
                if let Some(candidate) = catalog
                    .cached_media(anilist)
                    .await
                    .and_then(|cached| from_summary(&cached.value.summary, ids.clone()))
                {
                    own.push(candidate);
                } else if let Some(anime) = index
                    .entries()
                    .iter()
                    .find(|anime| anime.ids.anilist == Some(anilist))
                {
                    own.push(from_offline(anime));
                }
            }
            // The user's own shows come first so they win a tied score.
            own.extend(index.entries().iter().map(from_offline));
            Ok(own)
        })
    }
}

fn from_summary(summary: &metadata::MediaSummary, ids: Vec<ExternalId>) -> Option<Candidate> {
    let title = summary
        .title
        .romaji
        .as_ref()
        .or(summary.title.english.as_ref())
        .or(summary.title.native.as_ref())?;
    let mut candidate = Candidate::new(title.clone(), ids);
    for alias in [
        summary.title.english.as_ref(),
        summary.title.native.as_ref(),
        summary.title.romaji.as_ref(),
    ]
    .into_iter()
    .flatten()
    {
        candidate = candidate.with_alias(alias.clone());
    }
    Some(candidate)
}

fn from_offline(anime: &OfflineAnime) -> Candidate {
    let mut candidate = Candidate::new(anime.title.clone(), offline_ids(&anime.ids));
    for synonym in &anime.synonyms {
        candidate = candidate.with_alias(synonym.clone());
    }
    candidate
}

fn offline_ids(ids: &AnimeOfflineIds) -> Vec<ExternalId> {
    let mut external = Vec::with_capacity(4);
    if let Some(id) = ids.anilist {
        external.push(ExternalId::AniList(id));
    }
    if let Some(id) = ids.mal {
        external.push(ExternalId::Mal(id));
    }
    if let Some(id) = ids.anidb {
        external.push(ExternalId::AniDb(id));
    }
    if let Some(id) = ids.kitsu {
        external.push(ExternalId::Kitsu(id));
    }
    external
}

/// Converts the Library setting's percentage into a matcher threshold.
#[must_use]
pub(crate) fn threshold(percent: u8) -> f64 {
    f64::from(percent) / 100.0
}

/// Frees the one-scan slot when the task ends, including on error.
struct ScanGuard(Arc<AtomicBool>);

impl Drop for ScanGuard {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

/// Runs scans on the engine's Tokio runtime and publishes their progress.
#[derive(Debug, Clone)]
pub struct LibraryScanner {
    db: Db,
    candidates: Arc<Mutex<Arc<dyn CandidateSource>>>,
    events: Arc<Mutex<Option<Sender<AppEvent>>>>,
    hydration: Option<(Client, PosterCache)>,
    handle: Handle,
    running: Arc<AtomicBool>,
    /// The last scan requested while another was running, replayed on finish.
    pending: Arc<Mutex<Option<PendingScan>>>,
}

type PendingScan = (Vec<PathBuf>, f64, FillerPolicy);

impl LibraryScanner {
    /// A scanner without metadata hydration, for tests with a fake source.
    #[must_use]
    pub fn new(
        db: Db,
        candidates: Arc<dyn CandidateSource>,
        events: Arc<Mutex<Option<Sender<AppEvent>>>>,
        handle: Handle,
    ) -> Self {
        Self {
            db,
            candidates: Arc::new(Mutex::new(candidates)),
            events,
            hydration: None,
            handle,
            running: Arc::new(AtomicBool::new(false)),
            pending: Arc::new(Mutex::new(None)),
        }
    }

    /// A scanner that also hydrates matches with `AniZip` and filler lists.
    #[must_use]
    pub fn with_hydration(
        db: Db,
        candidates: Arc<dyn CandidateSource>,
        client: Client,
        disk: PosterCache,
        events: Arc<Mutex<Option<Sender<AppEvent>>>>,
        handle: Handle,
    ) -> Self {
        Self {
            hydration: Some((client, disk)),
            ..Self::new(db, candidates, events, handle)
        }
    }

    /// Swaps the candidate source, as when the active platform changes.
    pub fn set_candidates(&self, candidates: Arc<dyn CandidateSource>) {
        if let Ok(mut slot) = self.candidates.lock() {
            *slot = candidates;
        }
    }

    /// Schedules one scan. A second call while a scan runs is queued and
    /// refused with [`ScanError::AlreadyScanning`].
    ///
    /// # Errors
    ///
    /// [`ScanError::AlreadyScanning`] when a scan is in flight.
    pub fn start(
        &self,
        folders: Vec<PathBuf>,
        threshold: f64,
        filler: FillerPolicy,
    ) -> Result<(), ScanError> {
        if self
            .running
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            // Keep the newest folder change: the watcher can fire many times
            // while one scan is in flight, and only the last state matters.
            if let Ok(mut pending) = self.pending.lock() {
                *pending = Some((folders, threshold, filler));
            }
            return Err(ScanError::AlreadyScanning);
        }
        let scanner = self.clone();
        self.handle.spawn(async move {
            let result = {
                let _slot = ScanGuard(Arc::clone(&scanner.running));
                scanner.run(&folders, threshold, filler).await
            };
            scanner.publish_result(result);
            scanner.start_pending();
        });
        Ok(())
    }

    /// Starts the scan queued while the previous one was running, if any.
    fn start_pending(&self) {
        let pending = self
            .pending
            .lock()
            .map_or_else(|_| None, |mut slot| slot.take());
        if let Some((folders, threshold, filler)) = pending
            && let Err(error) = self.start(folders, threshold, filler)
        {
            debug!(%error, "queued scan could not start");
        }
    }

    /// Runs one scan in the caller's future, without the one-at-a-time slot.
    ///
    /// # Errors
    ///
    /// Any candidate or storage failure from [`library::Library::scan`].
    pub async fn run(
        &self,
        folders: &[PathBuf],
        threshold: f64,
        filler: FillerPolicy,
    ) -> Result<ScanSummary, EngineError> {
        let source = self.candidates.lock().map_or_else(
            |error| Err(EngineError::CandidateLock(error.to_string())),
            |slot| Ok(Arc::clone(&slot)),
        )?;
        let candidates = source.load().await?;
        let last = Arc::new(Mutex::new(
            Instant::now()
                .checked_sub(PROGRESS_INTERVAL)
                .unwrap_or_else(Instant::now),
        ));
        let events = self
            .events
            .lock()
            .map_or_else(|_| None, |slot| slot.clone());
        let mut library = Library::with_threshold(self.db.clone(), threshold);
        if let Some((client, disk)) = &self.hydration {
            let lookup = if filler == FillerPolicy::Enabled {
                FillerLookup::Enabled
            } else {
                FillerLookup::Disabled
            };
            library = library.hydrating(
                metadata::AniZip::new(client.clone(), disk.clone()),
                AnimeFillerList::new(client.clone(), disk.clone(), lookup),
            );
        }
        let summary = library
            .scan(folders, &candidates, |progress| {
                let Some(events) = &events else {
                    return;
                };
                let Ok(mut last) = last.lock() else {
                    return;
                };
                if last.elapsed() < PROGRESS_INTERVAL {
                    return;
                }
                *last = Instant::now();
                let _ = events.send(AppEvent::LibraryScanProgress {
                    files_walked: progress.files_walked,
                    files_matched: progress.files_matched,
                });
            })
            .await?;
        if let Some(events) = &events {
            // The throttle can drop the last intermediate update.
            let _ = events.send(AppEvent::LibraryScanProgress {
                files_walked: summary.files_walked,
                files_matched: summary.files_matched,
            });
        }
        Ok(summary)
    }

    fn publish_result(&self, result: Result<ScanSummary, EngineError>) {
        let (counts, error) = match &result {
            Ok(summary) => (
                (
                    summary.files_walked,
                    summary.files_matched,
                    summary.files_unmatched,
                    summary.shelved,
                ),
                None,
            ),
            Err(failure) => ((0, 0, 0, 0), Some(failure.to_string())),
        };
        if let Some(events) = self
            .events
            .lock()
            .map_or_else(|_| None, |slot| slot.clone())
        {
            let (files_walked, files_matched, files_unmatched, shelved) = counts;
            let _ = events.send(AppEvent::LibraryScanFinished {
                files_walked,
                files_matched,
                files_unmatched,
                shelved,
                error,
            });
            if result.is_ok() {
                let _ = events.send(AppEvent::LibraryChanged);
            } else if let Err(failure) = result {
                debug!(%failure, "library scan failed");
            }
        }
    }
}

/// The library rows belonging to one media, path ordered.
pub(crate) fn files_for(
    db: &Db,
    media: MediaId,
) -> BoxFuture<Result<Vec<LocalFile>, LibraryError>> {
    let db = db.clone();
    Box::pin(async move {
        let mut files: Vec<LocalFile> = db
            .local_file()
            .all()
            .await?
            .into_iter()
            .filter(|file| file.media_id == Some(media))
            .collect();
        files.sort_by(|left, right| left.path.cmp(&right.path));
        Ok(files)
    })
}

/// Every media that currently has at least one local file.
pub(crate) fn matched_media(db: &Db) -> BoxFuture<Result<Vec<MediaId>, LibraryError>> {
    let db = db.clone();
    Box::pin(async move {
        let mut media: Vec<MediaId> = db
            .local_file()
            .all()
            .await?
            .into_iter()
            .filter_map(|file| file.media_id)
            .collect();
        media.sort_by_key(|media| media.get());
        media.dedup();
        Ok(media)
    })
}

#[cfg(test)]
mod tests {
    use std::{fs, sync::mpsc};

    use tempfile::tempdir;
    use tokio::time::timeout;

    use super::*;

    #[derive(Debug)]
    struct FakeCandidates {
        titles: Vec<Candidate>,
        gate: Arc<Mutex<Option<mpsc::Receiver<()>>>>,
    }

    impl CandidateSource for FakeCandidates {
        fn load(&self) -> BoxFuture<Result<Vec<Candidate>, EngineError>> {
            let titles = self.titles.clone();
            let gate = Arc::clone(&self.gate);
            Box::pin(async move {
                if let Ok(mut slot) = gate.lock()
                    && let Some(receiver) = slot.take()
                {
                    let _ = receiver.recv();
                }
                Ok(titles)
            })
        }
    }

    fn scanner(
        db: Db,
        titles: Vec<Candidate>,
        gate: Option<mpsc::Receiver<()>>,
        events: Option<Sender<AppEvent>>,
    ) -> LibraryScanner {
        LibraryScanner::new(
            db,
            Arc::new(FakeCandidates {
                titles,
                gate: Arc::new(Mutex::new(gate)),
            }),
            Arc::new(Mutex::new(events)),
            Handle::current(),
        )
    }

    #[tokio::test]
    async fn a_scan_of_a_temp_folder_matches_and_publishes_its_result() {
        let dir = tempdir().expect("a temp directory");
        let folder = dir.path().join("shows");
        fs::create_dir_all(&folder).expect("the library folder");
        fs::write(folder.join("[Group] Series Alpha - 01 [1080p].mkv"), b"").expect("a release");
        fs::write(folder.join("[Group] Unknown Show - 01 [1080p].mkv"), b"").expect("an unknown");
        let db = Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("a temp database");
        let (events, receiver) = flume::unbounded();
        let scanner = scanner(
            db.clone(),
            vec![Candidate::new(
                "Series Alpha",
                vec![ExternalId::AniList(42)],
            )],
            None,
            Some(events),
        );

        scanner
            .start(vec![folder], 0.5, FillerPolicy::Disabled)
            .expect("the first scan starts");
        let (files_walked, files_matched, files_unmatched, error) =
            timeout(Duration::from_secs(5), async {
                loop {
                    let event = receiver.recv_async().await.expect("the channel stays open");
                    if let AppEvent::LibraryScanFinished {
                        files_walked,
                        files_matched,
                        files_unmatched,
                        error,
                        ..
                    } = event
                    {
                        return (files_walked, files_matched, files_unmatched, error);
                    }
                }
            })
            .await
            .expect("the scan finishes");
        assert_eq!(files_walked, 2);
        assert_eq!(files_matched, 1);
        assert_eq!(files_unmatched, 1);
        assert_eq!(error, None);

        let summary = db
            .scan_summary()
            .latest()
            .await
            .expect("the summary read")
            .expect("the scan wrote a summary");
        assert_eq!(summary.by_media.len(), 1);
        let media = summary.by_media.first().expect("one matched media").media;
        let files = files_for(&db, media).await.expect("the files read");
        assert_eq!(files.len(), 1, "only the matched file belongs to the media");
    }

    #[tokio::test]
    async fn a_second_scan_is_refused_while_one_is_running() {
        let dir = tempdir().expect("a temp directory");
        let folder = dir.path().join("shows");
        fs::create_dir_all(&folder).expect("the library folder");
        let db = Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("a temp database");
        let (release, gate) = mpsc::channel();
        let scanner = scanner(db, Vec::new(), Some(gate), None);

        scanner
            .start(vec![folder.clone()], 0.5, FillerPolicy::Disabled)
            .expect("the first scan starts");
        assert_eq!(
            scanner.start(vec![folder], 0.5, FillerPolicy::Disabled),
            Err(ScanError::AlreadyScanning)
        );

        let _ = release.send(());
    }
}
