//! The engine's torrent module: one lazily started session and loopback
//! relay shared by every torrent stream (#217, #179 Q3, ADR-0005/0006).
//!
//! Nothing starts until the first torrent stream or file list asks for it.
//! The relay then lives until the app quits. The session can be restarted
//! for a Network or Storage change (ADR-0010), but never under a live
//! stream: the restart waits until the last stream ends.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use domain::InfoHash;
use relay::Relay;
use torrent::{PeerDiscovery, RateLimits, TorrentSession, TorrentSessionOptions};
use tracing::warn;

use crate::network::{TorrentRestart, TorrentSettingsChange};

/// How the next session is started.
#[derive(Debug, Clone)]
struct Config {
    root: PathBuf,
    discovery: PeerDiscovery,
    options: TorrentSessionOptions,
}

/// The torrent session and relay behind torrent streams.
#[derive(Debug)]
pub struct TorrentStreams {
    config: Mutex<Config>,
    session: tokio::sync::Mutex<Option<Arc<TorrentSession>>>,
    relay: tokio::sync::OnceCell<Relay>,
    /// Infohashes with a stream preparing or playing.
    active: Mutex<HashSet<InfoHash>>,
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
            active: Mutex::new(HashSet::new()),
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

    /// The infohashes of streams preparing or playing. Clearing the torrent
    /// cache must leave these alone.
    #[must_use]
    pub fn active(&self) -> Vec<InfoHash> {
        lock(&self.active).iter().copied().collect()
    }

    /// Whether any torrent stream is preparing or playing.
    #[must_use]
    pub fn is_streaming(&self) -> bool {
        !lock(&self.active).is_empty()
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

    /// Marks `info_hash` as streaming.
    pub(crate) fn track(&self, info_hash: InfoHash) {
        lock(&self.active).insert(info_hash);
    }

    /// Ends the stream of `info_hash` (#179 Q6, #180): the torrent is
    /// paused, then a partial one is deleted and a complete one seeds. A
    /// restart deferred by a settings save runs once no stream is left.
    pub(crate) async fn end_stream(&self, info_hash: InfoHash) {
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
        let idle = {
            let mut active = lock(&self.active);
            active.remove(&info_hash);
            active.is_empty()
        };
        if idle && self.restart_pending.swap(false, Ordering::SeqCst) {
            self.restart(TorrentRestart::Now).await;
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
