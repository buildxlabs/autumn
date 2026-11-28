//! The engine's torrent module: one lazily started session and loopback
//! relay shared by every torrent stream (#217, #179 Q3, ADR-0005/0006).
//!
//! Nothing starts until the first torrent stream or file list asks for it.
//! The relay then lives until the app quits. The session can be restarted
//! for a Network or Storage change (ADR-0010), but never under a live
//! stream: the restart waits until the last stream ends.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use domain::{InfoHash, TorrentLink, TorrentListRow, TorrentOrigin};
use relay::Relay;
use torrent::{PeerDiscovery, RateLimits, TorrentSession, TorrentSessionOptions};
use tracing::warn;

use crate::{
    network::{TorrentRestart, TorrentSettingsChange},
    torrent_list::TorrentList,
};

/// How the next session is started.
#[derive(Debug, Clone)]
struct Config {
    root: PathBuf,
    discovery: PeerDiscovery,
    options: TorrentSessionOptions,
}

/// Why a torrent is held in the session (#208 reconciliation): a Torrent
/// List row shows a preload as `downloading` or `seeding` with origin
/// `preload`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamOrigin {
    /// A playback session is preparing or playing it.
    Playback,
    /// It holds the next episode's preload.
    Preload,
}

/// The torrent session and relay behind torrent streams.
#[derive(Debug)]
pub struct TorrentStreams {
    config: Mutex<Config>,
    session: tokio::sync::Mutex<Option<Arc<TorrentSession>>>,
    relay: tokio::sync::OnceCell<Relay>,
    /// Infohashes held by a stream or a preload, one origin per holder. A
    /// batch torrent can be held by the current stream and the preload of
    /// its next episode at once.
    active: Mutex<HashMap<InfoHash, Vec<StreamOrigin>>>,
    /// Why each torrent was added and, when known, its episode link.
    list: TorrentList,
    /// A restart was asked for while a stream played.
    restart_pending: AtomicBool,
}

impl TorrentStreams {
    /// Streams whose files land in `root`, found through `discovery`, with
    /// `options` (Network settings and the Storage upload toggle).
    #[must_use]
    pub fn new(root: PathBuf, discovery: PeerDiscovery, options: TorrentSessionOptions) -> Self {
        Self {
            config: Mutex::new(Config {
                root,
                discovery,
                options,
            }),
            session: tokio::sync::Mutex::new(None),
            relay: tokio::sync::OnceCell::new(),
            active: Mutex::new(HashMap::new()),
            list: TorrentList::default(),
            restart_pending: AtomicBool::new(false),
        }
    }

    /// The running session, started on first use.
    ///
    /// # Errors
    ///
    /// As [`TorrentSession::start`], for example a bound interface that is
    /// missing (fail closed, ADR-0010).
    pub async fn session(&self) -> Result<Arc<TorrentSession>, torrent::Error> {
        let mut slot = self.session.lock().await;
        if let Some(session) = slot.as_ref() {
            return Ok(Arc::clone(session));
        }
        let config = lock(&self.config).clone();
        let session =
            Arc::new(TorrentSession::start(config.root, config.discovery, config.options).await?);
        *slot = Some(Arc::clone(&session));
        Ok(session)
    }

    /// The loopback relay, started on first use.
    ///
    /// # Errors
    ///
    /// [`relay::Error::Bind`] if no loopback port can be bound.
    pub async fn relay(&self) -> Result<&Relay, relay::Error> {
        self.relay.get_or_try_init(Relay::start).await
    }

    /// The infohashes of streams preparing or playing and of the preload.
    /// Clearing the torrent cache must leave these alone.
    #[must_use]
    pub fn active(&self) -> Vec<InfoHash> {
        lock(&self.active).keys().copied().collect()
    }

    /// Why `info_hash` is held: [`StreamOrigin::Playback`] while a stream
    /// uses it, else [`StreamOrigin::Preload`], `None` when nothing holds it.
    #[must_use]
    pub fn origin(&self, info_hash: InfoHash) -> Option<StreamOrigin> {
        let active = lock(&self.active);
        let origins = active.get(&info_hash)?;
        if origins.contains(&StreamOrigin::Playback) {
            Some(StreamOrigin::Playback)
        } else {
            origins.first().copied()
        }
    }

    /// Whether any torrent stream or preload holds a torrent.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        !lock(&self.active).is_empty()
    }

    /// Whether `info_hash` is attached to the active playback session. With
    /// one session at a time, this is the row that shows as `streaming`.
    #[must_use]
    pub fn is_streaming_torrent(&self, info_hash: InfoHash) -> bool {
        self.origin(info_hash) == Some(StreamOrigin::Playback)
    }

    /// Remembers why a torrent was added and its episode link, when known.
    /// A playback stream registers itself when it starts; the preload and
    /// incoming paths call this with their own origin (#207 Q1, #208).
    pub fn register(&self, info_hash: InfoHash, origin: TorrentOrigin, link: Option<TorrentLink>) {
        self.list.register(info_hash, origin, link);
    }

    /// A fresh Torrent List snapshot, newest first. Sampling a session that
    /// has not started does not start one.
    ///
    /// # Errors
    ///
    /// As [`TorrentSession::list`].
    pub async fn rows(&self) -> Result<Vec<TorrentListRow>, torrent::Error> {
        let Some(session) = self.session.lock().await.clone() else {
            return Ok(Vec::new());
        };
        let streaming: HashSet<InfoHash> = lock(&self.active)
            .iter()
            .filter(|(_, origins)| origins.contains(&StreamOrigin::Playback))
            .map(|(info_hash, _)| *info_hash)
            .collect();
        Ok(self.list.rows(session.list()?, &streaming))
    }

    /// Removes a torrent and its cached files, running `end_session` first
    /// (#207 Q11). The caller ends the playback session if this torrent is
    /// streaming, so its relay token is revoked and no reader keeps buffering
    /// after the files are gone. A torrent that is merely seeding needs no
    /// session end and is removed outright.
    ///
    /// # Errors
    ///
    /// As [`TorrentSession::delete`].
    pub async fn remove(
        &self,
        info_hash: InfoHash,
        end_session: impl Future<Output = ()>,
    ) -> Result<(), torrent::Error> {
        end_session.await;
        self.list.unregister(info_hash);
        let Some(session) = self.session.lock().await.clone() else {
            return Ok(());
        };
        session.delete(info_hash).await
    }

    /// Applies a Network save (ADR-0010): rate limits change live, and a
    /// construction-time change restarts the session now or after the last
    /// stream ends, as `change` decided. The Storage upload toggle is kept.
    pub async fn apply_network(
        &self,
        change: &TorrentSettingsChange,
        mut options: TorrentSessionOptions,
    ) {
        {
            let mut config = lock(&self.config);
            options.disable_upload = config.options.disable_upload;
            config.options = options;
        }
        if change.rate_limits_changed {
            self.set_rate_limits(RateLimits::from_bytes_per_sec(
                change.download_limit_bytes_per_sec,
                change.upload_limit_bytes_per_sec,
            ))
            .await;
        }
        self.restart(change.restart).await;
    }

    /// Applies a Storage save: a new download folder or upload toggle
    /// restarts the session, deferred while a stream plays. `default_root`
    /// is used when the tab names no folder.
    pub async fn apply_storage(&self, torrent: &settings::TorrentSettings, default_root: PathBuf) {
        let root = torrent.download_folder.clone().unwrap_or(default_root);
        let disable_upload = torrent.disable_upload;
        let changed = {
            let mut config = lock(&self.config);
            let changed = config.root != root || config.options.disable_upload != disable_upload;
            config.root = root;
            config.options.disable_upload = disable_upload;
            changed
        };
        if changed {
            let restart = if self.is_streaming() {
                TorrentRestart::AfterPlaybackStops
            } else {
                TorrentRestart::Now
            };
            self.restart(restart).await;
        }
    }

    /// Holds `info_hash` for `origin`.
    pub(crate) fn track(&self, info_hash: InfoHash, origin: StreamOrigin) {
        lock(&self.active)
            .entry(info_hash)
            .or_default()
            .push(origin);
    }

    /// Releases one `origin` hold on `info_hash`. When it was the last one
    /// the stream ends (#179 Q6, #180): the torrent is paused, then a
    /// partial one is deleted and a complete one seeds. A torrent another
    /// holder still uses is left running. A restart deferred by a settings
    /// save runs once nothing is held.
    pub(crate) async fn end_stream(&self, info_hash: InfoHash, origin: StreamOrigin) {
        let last = {
            let mut active = lock(&self.active);
            let origins = active.entry(info_hash).or_default();
            if let Some(at) = origins.iter().position(|held| *held == origin) {
                origins.swap_remove(at);
            }
            let last = origins.is_empty();
            if last {
                active.remove(&info_hash);
            }
            last
        };
        if last {
            self.drop_torrent(info_hash).await;
        }
        if !self.is_streaming() && self.restart_pending.swap(false, Ordering::SeqCst) {
            self.restart(TorrentRestart::Now).await;
        }
    }

    /// Pauses `info_hash`, then deletes it when partial or lets it seed.
    async fn drop_torrent(&self, info_hash: InfoHash) {
        let session = self.session.lock().await.clone();
        if let Some(session) = session {
            // An error here means the torrent never made it into the session
            // (a cancel during metadata) or is already paused.
            let _ = session.pause(info_hash).await;
            match session.drop_stream(info_hash).await {
                Ok(()) | Err(torrent::Error::NotFound(_)) => {}
                Err(error) => warn!(%error, "could not drop a torrent stream"),
            }
        }
    }

    async fn set_rate_limits(&self, limits: RateLimits) {
        if let Some(session) = self.session.lock().await.as_ref() {
            session.set_rate_limits(limits);
        }
        lock(&self.config).options.rate_limits = limits;
    }

    /// Stops the session so the next stream starts one with the current
    /// config, or remembers to once no stream is left.
    async fn restart(&self, restart: TorrentRestart) {
        match restart {
            TorrentRestart::Now if !self.is_streaming() => {
                let previous = self.session.lock().await.take();
                if let Some(previous) = previous {
                    if let Ok(session) = Arc::try_unwrap(previous) {
                        if let Err(error) = session.shutdown().await {
                            warn!(%error, "the torrent session did not shut down cleanly");
                        }
                    } else {
                        // A file list still holds it; it stops when that ends.
                        warn!("restarting the torrent session while it is in use");
                    }
                }
            }
            TorrentRestart::Now | TorrentRestart::AfterPlaybackStops => {
                self.restart_pending.store(true, Ordering::SeqCst);
            }
            TorrentRestart::NotRequired => {}
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, SocketAddr},
        path::PathBuf,
        sync::{Arc, Mutex},
        time::Duration,
    };

    use domain::TorrentOrigin;
    use librqbit::{CreateTorrentOptions, ListenerOptions, Session, SessionOptions};
    use tempfile::TempDir;
    use torrent::{PeerDiscovery, TorrentSessionOptions, TorrentSource};

    use super::{StreamOrigin, TorrentStreams};

    /// A plain librqbit session seeding one small file over loopback, as in
    /// `crates/torrent/tests/session.rs`: no live network in the test.
    async fn seeder() -> (Arc<Session>, TempDir, Vec<u8>, SocketAddr) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ep.mkv");
        std::fs::write(&path, vec![0x5A; 1024 * 1024]).unwrap();
        let session = Session::new_with_opts(
            dir.path().into(),
            SessionOptions {
                dht: None,
                disable_local_service_discovery: true,
                disable_trackers: true,
                listen: Some(ListenerOptions {
                    listen_addr: (Ipv4Addr::LOCALHOST, 0).into(),
                    ..Default::default()
                }),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        let (created, handle) = session
            .create_and_serve_torrent(
                &path,
                CreateTorrentOptions {
                    piece_length: Some(256 * 1024),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        handle.wait_until_completed().await.unwrap();
        let addr = session.listen_addr().unwrap();
        let bytes = created.as_bytes().unwrap().to_vec();
        (session, dir, bytes, addr)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn removing_a_streaming_torrent_ends_the_session_before_deleting_files() {
        let (_seeder, _seed_dir, bytes, peer) = seeder().await;
        let cache = tempfile::tempdir().unwrap();
        let root: PathBuf = cache.path().to_path_buf();
        let streams = TorrentStreams::new(
            root,
            PeerDiscovery::Only(vec![peer]),
            TorrentSessionOptions::default(),
        );
        let session = streams.session().await.unwrap();
        let info_hash = session.add(TorrentSource::File(bytes), 0).await.unwrap();
        let dir = cache.path().join(info_hash.to_string());
        tokio::time::timeout(Duration::from_secs(30), async {
            while !session.stats(info_hash).is_ok_and(|stats| stats.finished) {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        streams.register(info_hash, TorrentOrigin::Stream, None);
        streams.track(info_hash, StreamOrigin::Playback);
        assert!(dir.exists(), "the complete file is cached before removal");

        let order = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&order);
        streams
            .remove(info_hash, async {
                seen.lock().unwrap().push("session-ended");
                assert!(
                    dir.exists(),
                    "the relay token is revoked before any file is deleted"
                );
                streams.end_stream(info_hash, StreamOrigin::Playback).await;
            })
            .await
            .unwrap();

        assert_eq!(*order.lock().unwrap(), ["session-ended"]);
        assert!(
            !dir.exists(),
            "removal deleted the cache after the session ended"
        );
        assert!(
            matches!(session.stats(info_hash), Err(torrent::Error::NotFound(_))),
            "the torrent left the session"
        );
        assert!(
            !streams.is_streaming_torrent(info_hash),
            "no dangling stream"
        );
    }
}
