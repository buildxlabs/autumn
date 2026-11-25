//! The scan-summaries screen's store (#134). Screens never call the engine
//! directly; this store owns the selection and the match/ignore commands.

use std::{collections::HashMap, path::PathBuf, sync::Arc, time::SystemTime};

use db::ScanSummary;
use domain::AppEvent;
use engine::{EngineServices, LibraryMediaInfo};
use gpui_kit::{Context, Task};

use crate::entry_store::call;

/// The audit log of recent scans, and the current selection in it.
pub struct ScanSummariesStore {
    services: Arc<dyn EngineServices>,
    summaries: Vec<ScanSummary>,
    /// The selected scan's start time: stable when a newer scan is inserted.
    selected: Option<SystemTime>,
    media: Vec<LibraryMediaInfo>,
    load_task: Option<Task<()>>,
    command_task: Option<Task<()>>,
    error: Option<String>,
    /// Edits shown immediately, until the next scan rewrites the summary.
    pending: HashMap<PathBuf, PendingEdit>,
}

/// An edit applied optimistically to an unmatched file's row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PendingEdit {
    /// The file was matched to an AniList media.
    Matched(u32),
    /// The file was marked ignored.
    Ignored,
}

impl std::fmt::Debug for ScanSummariesStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ScanSummariesStore")
            .field("summaries", &self.summaries.len())
            .field("selected", &self.selected)
            .field("media", &self.media.len())
            .finish_non_exhaustive()
    }
}

impl ScanSummariesStore {
    /// A store that reads and updates the library through `services`.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services,
            summaries: Vec::new(),
            selected: None,
            media: Vec::new(),
            load_task: None,
            command_task: None,
            error: None,
            pending: HashMap::new(),
        }
    }

    /// Every stored summary, newest first.
    #[must_use]
    pub fn summaries(&self) -> &[ScanSummary] {
        &self.summaries
    }

    /// The summary currently shown, if one is selected.
    #[must_use]
    pub fn selected(&self) -> Option<&ScanSummary> {
        let at = self.selected?;
        self.summaries
            .iter()
            .find(|summary| summary.started_at == at)
    }

    /// The index of the selected summary, for highlighting the list row.
    #[must_use]
    pub fn selected_index(&self) -> Option<usize> {
        let at = self.selected?;
        self.summaries
            .iter()
            .position(|summary| summary.started_at == at)
    }

    /// Edits the user just made, shown before the next scan records them.
    #[must_use]
    pub fn pending(&self) -> &HashMap<PathBuf, PendingEdit> {
        &self.pending
    }

    /// Takes an error from the last load or command, once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// The media with local files, for Entry links and display titles.
    #[must_use]
    pub fn media(&self) -> &[LibraryMediaInfo] {
        &self.media
    }

    /// The AniList id and title of one media, when known.
    #[must_use]
    pub fn info(&self, media: domain::MediaId) -> Option<&LibraryMediaInfo> {
        self.media.iter().find(|info| info.media == media)
    }

    /// Selects a summary by its position in the list.
    pub fn select(&mut self, index: usize, cx: &mut Context<'_, Self>) {
        if let Some(summary) = self.summaries.get(index)
            && self.selected != Some(summary.started_at)
        {
            self.selected = Some(summary.started_at);
            cx.notify();
        }
    }

    /// Loads the summaries and media on first open.
    pub fn open(&mut self, cx: &mut Context<'_, Self>) {
        self.reload(cx);
    }

    /// Applies a scan event from the foreground pump.
    pub fn apply(&mut self, event: &AppEvent, cx: &mut Context<'_, Self>) {
        if matches!(
            event,
            AppEvent::LibraryScanFinished { .. } | AppEvent::LibraryChanged
        ) {
            if matches!(event, AppEvent::LibraryScanFinished { .. }) {
                self.pending.clear();
            }
            self.reload(cx);
        }
    }

    /// Matches one unmatched file to AniList media and reloads the log.
    pub fn match_file(
        &mut self,
        path: PathBuf,
        anilist_id: u32,
        episode: Option<u32>,
        cx: &mut Context<'_, Self>,
    ) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            let result = call(
                cx,
                services.manual_match(std::slice::from_ref(&path), anilist_id, episode),
            )
            .await;
            let _ = store.update(cx, |store, cx| match result {
                Ok(_) => {
                    store.pending.insert(path, PendingEdit::Matched(anilist_id));
                    store.error = None;
                    store.reload(cx);
                }
                Err(error) => {
                    store.error = Some(error);
                    cx.notify();
                }
            });
        }));
    }

    /// Ignores one unmatched file and reloads the log.
    pub fn ignore(&mut self, path: PathBuf, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.command_task = Some(cx.spawn(async move |store, cx| {
            let result = call(cx, services.ignore_file(&path)).await;
            let _ = store.update(cx, |store, cx| match result {
                Ok(_) => {
                    store.pending.insert(path, PendingEdit::Ignored);
                    store.error = None;
                    store.reload(cx);
                }
                Err(error) => {
                    store.error = Some(error);
                    cx.notify();
                }
            });
        }));
    }

    fn reload(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.load_task = Some(cx.spawn(async move |store, cx| {
            let Ok(summaries) = call(cx, services.summaries()).await else {
                return;
            };
            let Ok(media) = call(cx, services.library_media_info()).await else {
                return;
            };
            let _ = store.update(cx, |store, cx| {
                store.summaries.clone_from(&summaries);
                store.media = media;
                if store
                    .selected
                    .is_none_or(|at| !summaries.iter().any(|s| s.started_at == at))
                {
                    store.selected = summaries.first().map(|summary| summary.started_at);
                }
                cx.notify();
            });
        }));
    }
}

#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use domain::MediaId;
    use gpui_kit::{AppContext, TestAppContext};
    use tokio::runtime::Builder;

    use crate::{bridge, test_support::LibraryCommand, test_support::LibraryServices};

    use super::*;

    fn summary(matched: usize) -> ScanSummary {
        ScanSummary {
            started_at: SystemTime::now(),
            duration: Duration::from_secs(2),
            folders: Vec::new(),
            files_walked: 3,
            files_matched: matched,
            files_unmatched: 1,
            locked: 0,
            ignored: 0,
            shelved: 1,
            by_media: Vec::new(),
            unmatched: Vec::new(),
        }
    }

    /// The reloads go through the Tokio bridge, which can settle a beat after
    /// the foreground task when the whole suite runs in parallel.
    fn settle(cx: &mut TestAppContext, store: &gpui_kit::Entity<ScanSummariesStore>) {
        for _ in 0..50 {
            cx.run_until_parked();
            if store.read_with(cx, |store, _| !store.summaries().is_empty()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[gpui_kit::test]
    fn selection_and_match_ignore_commands_reach_the_engine(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .expect("a test runtime");
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = LibraryServices::default()
            .with_summaries(vec![summary(1), summary(2)])
            .with_media(vec![LibraryMediaInfo::new(
                MediaId::new(9),
                Some(42),
                Some("Series Alpha".to_owned()),
            )]);
        let calls = services.clone();
        let store = cx.new(|_| ScanSummariesStore::new(Arc::new(services)));

        cx.update(|cx| store.update(cx, ScanSummariesStore::open));
        settle(cx, &store);
        assert_eq!(store.read_with(cx, |store, _| store.summaries().len()), 2);
        assert_eq!(
            store.read_with(cx, |store, _| store.selected_index()),
            Some(0)
        );
        assert_eq!(
            store.read_with(cx, |store, _| store.selected().map(|s| s.files_matched)),
            Some(1)
        );

        cx.update(|cx| store.update(cx, |store, cx| store.select(1, cx)));
        assert_eq!(
            store.read_with(cx, |store, _| store.selected().map(|s| s.files_matched)),
            Some(2),
            "the newer summary is shown after selection"
        );
        assert_eq!(
            store.read_with(cx, |store, _| store
                .info(MediaId::new(9))
                .and_then(|i| i.anilist_id)),
            Some(42),
            "matched media link to their Entry page"
        );

        let path = PathBuf::from("/shows/unknown.mkv");
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.match_file(path.clone(), 42, Some(3), cx);
            });
        });
        cx.run_until_parked();
        cx.update(|cx| {
            store.update(cx, |store, cx| store.ignore(path.clone(), cx));
        });
        cx.run_until_parked();
        assert_eq!(
            calls.commands(),
            [
                LibraryCommand::Match {
                    path: path.clone(),
                    anilist_id: 42,
                    episode: Some(3),
                },
                LibraryCommand::Ignore(path),
            ]
        );
    }

    #[gpui_kit::test]
    fn selection_survives_a_newer_scan_at_the_top(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .expect("a test runtime");
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let older = summary(2);
        let services = LibraryServices::default().with_summaries(vec![summary(1), older.clone()]);
        let calls = services.clone();
        let store = cx.new(|_| ScanSummariesStore::new(Arc::new(services)));

        cx.update(|cx| store.update(cx, ScanSummariesStore::open));
        settle(cx, &store);
        cx.update(|cx| store.update(cx, |store, cx| store.select(1, cx)));
        assert_eq!(
            store.read_with(cx, |store, _| store.selected().map(|s| s.files_matched)),
            Some(2)
        );

        calls.set_summaries(vec![summary(3), summary(1), older]);
        cx.update(|cx| {
            store.update(cx, |store, cx| store.apply(&AppEvent::LibraryChanged, cx));
        });
        for _ in 0..50 {
            cx.run_until_parked();
            if store.read_with(cx, |store, _| store.summaries().len()) == 3 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }

        assert_eq!(
            store.read_with(cx, |store, _| store.selected().map(|s| s.files_matched)),
            Some(2),
            "the same scan stays selected after a newer one is inserted"
        );
        assert_eq!(
            store.read_with(cx, |store, _| store.selected_index()),
            Some(2),
            "the highlighted row moved with it"
        );
    }
}
