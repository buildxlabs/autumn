//! Scan state and the local library mirrored for screens (ticket #127).
//!
//! The store never scans itself: it applies engine events from the foreground
//! pump and sends commands through the bridge, so `render()` stays pure
//! (ARCHITECTURE §4).

use std::{collections::HashSet, sync::Arc};

use db::{LocalFile, ScanSummary};
use domain::{AppEvent, MediaId};
use engine::EngineServices;
use gpui_kit::{Context, Task, WeakEntity};

use crate::entry_store::call;

/// Progress within one scan, as last reported by the engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScanState {
    /// No scan is running.
    Idle,
    /// A scan is running; the counts are throttled by the engine to 4 Hz.
    Running {
        /// Files walked so far.
        files_walked: usize,
        /// Files matched so far.
        files_matched: usize,
    },
}

/// The counts of a finished scan, for the summary toast.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScanOutcome {
    /// Files found by the walk.
    pub files_walked: usize,
    /// Files matched to a media.
    pub files_matched: usize,
    /// Files left without a media.
    pub files_unmatched: usize,
    /// Files on a disconnected drive.
    pub shelved: usize,
    /// Why the scan failed, when it did.
    pub error: Option<String>,
}

/// The library as the screens see it, fed by engine events.
pub struct LibraryStore {
    services: Arc<dyn EngineServices>,
    scan: ScanState,
    summary: Option<ScanSummary>,
    files: Vec<LocalFile>,
    media: Vec<MediaId>,
    /// The last completion, taken once by the shell to show a toast.
    finish: Option<ScanOutcome>,
    /// Why a scan request was refused, surfaced by the shell as a toast.
    scan_error: Option<String>,
    scan_task: Option<Task<()>>,
    load_task: Option<Task<()>>,
}

impl std::fmt::Debug for LibraryStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LibraryStore")
            .field("scan", &self.scan)
            .field("summary", &self.summary)
            .field("media", &self.media)
            .finish_non_exhaustive()
    }
}

impl LibraryStore {
    /// A store that sends library commands through `services`.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services,
            scan: ScanState::Idle,
            summary: None,
            files: Vec::new(),
            media: Vec::new(),
            finish: None,
            scan_error: None,
            scan_task: None,
            load_task: None,
        }
    }

    /// Progress of the current scan.
    #[must_use]
    pub fn scan(&self) -> ScanState {
        self.scan
    }

    /// The newest recorded scan summary, after [`Self::refresh`] completes.
    #[must_use]
    pub fn summary(&self) -> Option<&ScanSummary> {
        self.summary.as_ref()
    }

    /// Every media with at least one local file.
    #[must_use]
    pub fn media(&self) -> &[MediaId] {
        &self.media
    }

    /// The local files matched to `media`, path ordered.
    #[must_use]
    pub fn files(&self, media: MediaId) -> Vec<LocalFile> {
        self.files
            .iter()
            .filter(|file| file.media_id == Some(media))
            .cloned()
            .collect()
    }

    /// Hands the last completion to the shell, once, for the summary toast.
    pub fn take_finish(&mut self) -> Option<ScanOutcome> {
        self.finish.take()
    }

    /// Hands a refused scan request's error to the shell, once.
    pub fn take_scan_error(&mut self) -> Option<String> {
        self.scan_error.take()
    }

    /// Applies an event from the foreground pump.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        match event {
            AppEvent::LibraryScanProgress {
                files_walked,
                files_matched,
            } => {
                self.scan = ScanState::Running {
                    files_walked,
                    files_matched,
                };
                cx.notify();
            }
            AppEvent::LibraryScanFinished {
                files_walked,
                files_matched,
                files_unmatched,
                shelved,
                error,
            } => {
                self.scan = ScanState::Idle;
                // Hold the toast until the fresh summary is loaded, so its
                // media count is the one from this scan.
                self.finish = None;
                cx.notify();
                self.refresh_with(
                    Some(ScanOutcome {
                        files_walked,
                        files_matched,
                        files_unmatched,
                        shelved,
                        error,
                    }),
                    cx,
                );
            }
            _ => {}
        }
    }

    /// Requests one manual scan. The engine refuses a second one while a scan
    /// is running; the resulting events drive this store.
    pub fn start_scan(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        let store: WeakEntity<Self> = cx.entity().downgrade();
        self.scan_error = None;
        self.scan_task = Some(cx.spawn(async move |_, cx| {
            let result = call(cx, services.start_scan()).await;
            let _ = store.update(cx, |store, cx| match result {
                Ok(()) => {
                    // Progress events can be delayed until the offline index
                    // loads, so show a spinning state immediately.
                    store.scan = ScanState::Running {
                        files_walked: 0,
                        files_matched: 0,
                    };
                    cx.notify();
                }
                Err(message) => {
                    store.scan = ScanState::Idle;
                    store.scan_error = Some(message);
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    /// Reloads the summary, media and files from the engine.
    pub fn refresh(&mut self, cx: &mut Context<'_, Self>) {
        self.refresh_with(None, cx);
    }

    fn refresh_with(&mut self, finish: Option<ScanOutcome>, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.load_task = Some(cx.spawn(async move |store, cx| {
            let summary = call(cx, services.latest_scan_summary())
                .await
                .ok()
                .flatten();
            // One read of the file table feeds both the media list and the
            // per-media views; a failed read keeps the last good library.
            let Ok(mut files) = call(cx, services.all_library_files()).await else {
                return;
            };
            files.sort_by(|left, right| left.path.cmp(&right.path));
            // A media with several episodes appears once, at its first path.
            let mut seen = HashSet::new();
            let media: Vec<MediaId> = files
                .iter()
                .filter_map(|file| file.media_id)
                .filter(|media| seen.insert(*media))
                .collect();
            let _ = store.update(cx, |store, cx| {
                if summary.is_some() {
                    store.summary = summary;
                }
                store.media = media;
                store.files = files;
                store.finish = finish;
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use gpui_kit::{AppContext, TestAppContext};
    use tokio::runtime::Builder;

    use crate::{bridge, test_support::LibraryServices};

    use super::*;

    fn summary() -> ScanSummary {
        ScanSummary {
            started_at: SystemTime::now(),
            duration: Duration::from_secs(2),
            folders: Vec::new(),
            files_walked: 5,
            files_matched: 3,
            files_unmatched: 1,
            locked: 0,
            ignored: 0,
            shelved: 1,
            by_media: Vec::new(),
            unmatched: Vec::new(),
        }
    }

    fn file(path: &str, media: MediaId) -> LocalFile {
        LocalFile {
            path: path.into(),
            size: 0,
            modified: 0,
            title: None,
            group: None,
            season: None,
            episode: None,
            kind: "episode".to_owned(),
            media_id: Some(media),
            confidence: Some(0.8),
            locked: false,
            ignored: false,
            filler: false,
            shelved: false,
        }
    }

    #[gpui_kit::test]
    fn progress_and_finish_events_drive_the_store(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .expect("a test runtime");
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let media = MediaId::new(3);
        let services = LibraryServices::new(
            summary(),
            vec![file("/shows/ep1.mkv", media), file("/shows/ep2.mkv", media)],
        );
        let calls = services.clone();
        let store = cx.new(|_| LibraryStore::new(Arc::new(services)));

        cx.update(|cx| store.update(cx, LibraryStore::start_scan));
        for _ in 0..50 {
            cx.run_until_parked();
            if store.read_with(cx, |store, _| store.scan()) != ScanState::Idle {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(calls.scan_count(), 1, "the scan reached the engine");
        assert_eq!(
            store.read_with(cx, |store, _| store.scan()),
            ScanState::Running {
                files_walked: 0,
                files_matched: 0
            },
            "the store shows progress before the first engine event"
        );

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(
                    AppEvent::LibraryScanProgress {
                        files_walked: 4,
                        files_matched: 2,
                    },
                    cx,
                );
            });
        });
        assert_eq!(
            store.read_with(cx, |store, _| store.scan()),
            ScanState::Running {
                files_walked: 4,
                files_matched: 2
            }
        );

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(
                    AppEvent::LibraryScanFinished {
                        files_walked: 5,
                        files_matched: 3,
                        files_unmatched: 1,
                        shelved: 1,
                        error: None,
                    },
                    cx,
                );
            });
        });
        // The summary is loaded through the Tokio bridge, so it can land a
        // little after the foreground task is parked under parallel tests.
        for _ in 0..50 {
            cx.run_until_parked();
            if store.read_with(cx, |store, _| store.summary().is_some()) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            store.read_with(cx, |store, _| store.scan()),
            ScanState::Idle
        );
        let outcome = cx
            .update(|cx| store.update(cx, |store, _| store.take_finish()))
            .expect("a finish event was recorded");
        assert_eq!(outcome.files_matched, 3);
        assert_eq!(outcome.error, None);
        assert_eq!(
            store.read_with(cx, |store, _| store.summary().map(|s| s.files_walked)),
            Some(5),
            "the summary was reloaded from the engine"
        );
        assert_eq!(
            store.read_with(cx, |store, _| store.media().to_vec()),
            [media]
        );
        assert_eq!(store.read_with(cx, |store, _| store.files(media).len()), 2);
        assert_eq!(
            cx.update(|cx| store.update(cx, |store, _| store.take_finish())),
            None,
            "the toast is handed out once"
        );
    }
}
