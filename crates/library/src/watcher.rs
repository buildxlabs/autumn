//! Watches library folders and batches file events into one auto-scan.
//!
//! The OS watcher feeds a channel; a task waits for ten seconds of quiet, then
//! reports each affected library folder once. Dropping or stopping the watcher
//! ends that task, which is how turning auto-scan off takes effect without a
//! restart.

use std::{fmt, path::PathBuf, time::Duration};

use flume::{Receiver, Sender};
use notify::{Event, EventKind, RecursiveMode, Watcher as NotifyWatcher};
use tokio::task::JoinHandle;

use crate::LibraryError;

/// Quiet time before a burst of events becomes one scan.
pub(crate) const DEFAULT_QUIET: Duration = Duration::from_secs(10);

/// A file watcher that reports the affected library folders after each quiet
/// period. The callback runs on the Tokio task that owns the debounce timer.
pub struct AutoScan {
    _watcher: notify::RecommendedWatcher,
    stop: Sender<()>,
    task: JoinHandle<()>,
}

impl fmt::Debug for AutoScan {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.debug_struct("AutoScan").finish_non_exhaustive()
    }
}

impl AutoScan {
    /// Watches `folders` recursively with the default ten-second debounce.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Watcher`] when a folder cannot be watched.
    pub fn start(
        folders: &[PathBuf],
        handle: &tokio::runtime::Handle,
        on_change: impl FnMut(Vec<PathBuf>) + Send + 'static,
    ) -> Result<Self, LibraryError> {
        Self::with_quiet(folders, DEFAULT_QUIET, handle, on_change)
    }

    /// A watcher with a caller-supplied quiet period, for tests and settings.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Watcher`] when a folder cannot be watched.
    pub fn with_quiet(
        folders: &[PathBuf],
        quiet: Duration,
        handle: &tokio::runtime::Handle,
        on_change: impl FnMut(Vec<PathBuf>) + Send + 'static,
    ) -> Result<Self, LibraryError> {
        let (events, receiver) = flume::unbounded();
        let (stop, stopped) = flume::bounded(1);
        let roots: Vec<PathBuf> = folders.to_vec();
        let mut watcher = notify::recommended_watcher(move |result: notify::Result<Event>| {
            let Ok(event) = result else {
                return;
            };
            if matches!(event.kind, EventKind::Access(_) | EventKind::Other) {
                return;
            }
            for path in event.paths {
                let folder = roots
                    .iter()
                    .find(|root| path.starts_with(root))
                    .cloned()
                    .unwrap_or(path);
                let _ = events.send(folder);
            }
        })?;
        for folder in folders {
            if !folder.is_dir() {
                // A disconnected drive has nothing to watch now; its files are
                // shelved by the scan that notices its absence.
                tracing::warn!(folder = %folder.display(), "not watching a missing library folder");
                continue;
            }
            if let Err(error) = watcher.watch(folder, RecursiveMode::Recursive) {
                tracing::warn!(%error, folder = %folder.display(), "could not watch a library folder");
            }
        }
        let task = handle.spawn(async move {
            debounce(receiver, stopped, quiet, on_change).await;
        });
        Ok(Self {
            _watcher: watcher,
            stop,
            task,
        })
    }

    /// Stops the watcher and its debounce task.
    pub fn stop(self) {
        let _ = self.stop.send(());
        self.task.abort();
    }
}

/// Collects events until nothing arrives for `quiet`, then emits one batch.
pub(crate) async fn debounce(
    events: Receiver<PathBuf>,
    stop: Receiver<()>,
    quiet: Duration,
    mut on_change: impl FnMut(Vec<PathBuf>),
) {
    let mut pending: Vec<PathBuf> = Vec::new();
    loop {
        tokio::select! {
            _ = stop.recv_async() => break,
            received = events.recv_async() => {
                let Ok(path) = received else {
                    break;
                };
                // A burst in one folder is one scan of that folder.
                if !pending.contains(&path) {
                    pending.push(path);
                }
            }
            () = tokio::time::sleep(quiet), if !pending.is_empty() => {
                on_change(std::mem::take(&mut pending));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use super::debounce;

    #[tokio::test]
    async fn a_burst_of_events_produces_one_batch() {
        let (events, receiver) = flume::unbounded();
        let (stop, stopped) = flume::bounded(1);
        let batches = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&batches);
        let task = tokio::spawn(async move {
            debounce(receiver, stopped, Duration::from_millis(40), move |batch| {
                seen.lock().unwrap().push(batch);
            })
            .await;
        });
        for _ in 0..50 {
            events.send(PathBuf::from("/library/show-01.mkv")).unwrap();
        }
        tokio::time::sleep(Duration::from_millis(120)).await;

        {
            let seen = batches.lock().unwrap();
            assert_eq!(seen.len(), 1, "the burst became one batch");
            assert_eq!(
                seen.first().map(Vec::len),
                Some(1),
                "fifty events in one folder produce one path"
            );
        }
        let _ = stop.send(());
        task.await.unwrap();
    }

    #[tokio::test]
    async fn stopping_ends_the_watcher_task() {
        let (events, receiver) = flume::unbounded();
        let (stop, stopped) = flume::bounded(1);
        let batches = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&batches);
        let task = tokio::spawn(async move {
            debounce(receiver, stopped, Duration::from_millis(20), move |batch| {
                seen.lock().unwrap().push(batch);
            })
            .await;
        });
        let _ = stop.send(());
        task.await.unwrap();
        let _ = events.send(PathBuf::from("/library/new.mkv"));
        tokio::time::sleep(Duration::from_millis(60)).await;

        assert!(batches.lock().unwrap().is_empty(), "no scan after stopping");
    }
}
