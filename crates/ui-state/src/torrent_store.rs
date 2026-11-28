//! The Torrent List's mirror of the session snapshot (#207 Q12).
//!
//! The engine samples librqbit and publishes whole snapshots at 4 Hz. The
//! store only replaces its rows when a snapshot differs and never polls:
//! actions go through the bridge so `render()` stays pure (ARCHITECTURE §4).

use std::sync::Arc;

use domain::{AppEvent, InfoHash, TorrentListRow, active_torrent_count};
use engine::EngineServices;
use gpui_kit::{Context, Task};

use crate::entry_store::call;

/// The rows and actions of the Torrent List.
pub struct TorrentStore {
    services: Option<Arc<dyn EngineServices>>,
    rows: Vec<TorrentListRow>,
    load: Option<Task<()>>,
    action: Option<Task<()>>,
    error: Option<String>,
}

impl std::fmt::Debug for TorrentStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TorrentStore")
            .field("rows", &self.rows.len())
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl TorrentStore {
    /// A store that calls engine services through the bridge.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services: Some(services),
            rows: Vec::new(),
            load: None,
            action: None,
            error: None,
        }
    }

    /// A store with no engine behind it, used until the composition root
    /// supplies one. It serves an empty list and ignores actions.
    #[must_use]
    pub fn inert() -> Self {
        Self {
            services: None,
            rows: Vec::new(),
            load: None,
            action: None,
            error: None,
        }
    }

    /// The current snapshot, newest first.
    #[must_use]
    pub fn rows(&self) -> &[TorrentListRow] {
        &self.rows
    }

    /// The rail's active-torrents count: every non-paused row, hidden at zero.
    #[must_use]
    pub fn active_count(&self) -> u32 {
        active_torrent_count(&self.rows)
    }

    /// A failure from the last action, until it is read and shown.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Takes the last action failure, so it is shown only once.
    pub fn take_error(&mut self) -> Option<String> {
        self.error.take()
    }

    /// Requests one snapshot. A newer request cancels an older one.
    pub fn open(&mut self, cx: &mut Context<'_, Self>) {
        let Some(services) = self.services.as_ref().map(Arc::clone) else {
            return;
        };
        self.load =
            Some(cx.spawn(
                async move |store, cx| match call(cx, services.torrent_rows()).await {
                    Ok(rows) => {
                        let _ = store.update(cx, |store, cx| {
                            if store.rows != rows {
                                store.rows = rows;
                                cx.notify();
                            }
                        });
                    }
                    Err(message) => {
                        let _ = store.update(cx, |store, cx| {
                            store.error = Some(message);
                            cx.notify();
                        });
                    }
                },
            ));
    }

    /// Applies a 4 Hz snapshot from the foreground pump.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        if let AppEvent::TorrentList(rows) = event
            && rows != self.rows
        {
            self.rows = rows;
            cx.notify();
        }
    }

    /// Pauses a torrent, keeping its cached files.
    pub fn pause(&mut self, info_hash: InfoHash, cx: &mut Context<'_, Self>) {
        self.command(info_hash, cx, |services, hash| services.torrent_pause(hash));
    }

    /// Resumes a paused torrent.
    pub fn resume(&mut self, info_hash: InfoHash, cx: &mut Context<'_, Self>) {
        self.command(info_hash, cx, |services, hash| {
            services.torrent_resume(hash)
        });
    }

    /// Removes a torrent and its cached files, ending a streaming session
    /// first. The engine owns that ordering (#207 Q11).
    pub fn remove(&mut self, info_hash: InfoHash, cx: &mut Context<'_, Self>) {
        self.command(info_hash, cx, |services, hash| {
            services.torrent_remove(hash)
        });
    }

    fn command(
        &mut self,
        info_hash: InfoHash,
        cx: &mut Context<'_, Self>,
        command: impl Fn(
            &Arc<dyn EngineServices>,
            InfoHash,
        ) -> engine::BoxFuture<Result<(), engine::EngineError>>
        + Send
        + 'static,
    ) {
        let Some(services) = self.services.as_ref().map(Arc::clone) else {
            return;
        };
        self.action = Some(cx.spawn(async move |store, cx| {
            if let Err(message) = call(cx, command(&services, info_hash)).await {
                let _ = store.update(cx, |store, cx| {
                    store.error = Some(message);
                    cx.notify();
                });
            }
        }));
    }
}

#[cfg(test)]
mod tests {
    use domain::{
        AppEvent, InfoHash, TorrentFileRow, TorrentLink, TorrentListRow, TorrentOrigin,
        TorrentState,
    };
    use gpui_kit::{AppContext, TestAppContext};

    use super::TorrentStore;

    fn row(byte: u8, state: TorrentState) -> TorrentListRow {
        TorrentListRow {
            info_hash: InfoHash::from_bytes([byte; 20]),
            name: "ep.mkv".to_owned(),
            origin: TorrentOrigin::Stream,
            state,
            selected_done_bytes: 50,
            selected_bytes: 100,
            download_bps: 0,
            upload_bps: 0,
            peers: 0,
            ratio_permille: 0,
            link: Some(TorrentLink::new(domain::MediaId::new(1), 3, Some(7))),
            files: vec![TorrentFileRow::new(0, "ep.mkv", 100, false)],
            magnet: String::new(),
        }
    }

    #[gpui_kit::test]
    fn the_badge_counts_live_rows_and_hides_at_zero(cx: &mut TestAppContext) {
        let store = cx.new(|_| TorrentStore::inert());
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(
                    AppEvent::TorrentList(vec![
                        row(1, TorrentState::Downloading),
                        row(2, TorrentState::Seeding),
                        row(3, TorrentState::Streaming),
                        row(4, TorrentState::Paused),
                    ]),
                    cx,
                );
            });
        });
        assert_eq!(
            store.read_with(cx, |store, _| store.active_count()),
            3,
            "paused rows are not active torrents"
        );

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(
                    AppEvent::TorrentList(vec![row(4, TorrentState::Paused)]),
                    cx,
                );
            });
        });
        assert_eq!(
            store.read_with(cx, |store, _| store.active_count()),
            0,
            "the rail badge is hidden"
        );
    }

    #[gpui_kit::test]
    fn snapshots_are_mirrored_only_when_they_change(cx: &mut TestAppContext) {
        let store = cx.new(|_| TorrentStore::inert());
        let snapshot = vec![row(1, TorrentState::Downloading)];
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::TorrentList(snapshot.clone()), cx);
                assert_eq!(store.rows().len(), 1);
            });
        });
        let notified = std::rc::Rc::new(std::cell::Cell::new(0));
        cx.update(|cx| {
            let notified = std::rc::Rc::clone(&notified);
            cx.observe(&store, move |_, _| notified.set(notified.get() + 1))
                .detach();
        });
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::TorrentList(snapshot), cx);
            });
        });
        assert_eq!(
            notified.get(),
            0,
            "an identical snapshot must not redraw the rows"
        );
    }
}
