//! Torrent streams in the playback manager (#217, #179, #180).
//!
//! ```text
//! play(Torrent { info_hash, file_index })
//!   Preparing: add with only_files → readiness gate, reported at 4 Hz
//!   mint a relay URL → launch the player on it (mpv network-timeout 300 s)
//!   Running: the usual session loop, plus the Buffering flag from the relay
//!            and the disk read switch once the file is complete
//!   end: revoke the token, pause, drop the stream (partial deleted,
//!        complete seeds)
//! ```
//!
//! A stop during `Preparing` drops the request and the torrent.
//!
//! Auto-next to another torrent stream (#179 Q5) stays in the session: the
//! new stream is held before the old one ends, so a shared batch torrent
//! is never dropped, then it prepares and the player loads its new URL.

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use domain::{AppEvent, InfoHash, PieceProgress, PrepareStage, Preparing, StreamSource};
use relay::{BufferingEvent, StreamSpec, StreamToken};
use tokio::sync::{broadcast, mpsc, oneshot};
use torrent::{DiskFile, FileStream, TorrentSession, TorrentSource};
use tracing::{debug, warn};

use super::{
    PlayRequest, PlaybackError, PlaybackManager, PlaybackSession, SessionControl, SessionState,
    Switch, lock,
};
use crate::{StreamOrigin, TorrentStreams};

/// The `Preparing` report rate: 4 Hz (ARCHITECTURE §4).
pub(super) const PREPARE_TICK: Duration = Duration::from_millis(250);
/// How long with no peer before the overlay says so (#179 Q2).
pub(super) const NO_PEERS_AFTER: Duration = Duration::from_secs(60);
/// mpv's `network-timeout` for a torrent stream (#179 Q4).
pub(super) const NETWORK_TIMEOUT: Duration = Duration::from_secs(300);
/// A torrent EOF counts as the end only within the last 2 % (#179 Q4).
const EOF_FRACTION: f64 = 0.98;

/// The running session's torrent stream.
#[derive(Debug)]
pub(super) struct LiveStream {
    streams: Arc<TorrentStreams>,
    session: Arc<TorrentSession>,
    info_hash: InfoHash,
    token: StreamToken,
    buffering: broadcast::Receiver<BufferingEvent>,
    /// The relay reads the finished file from disk.
    on_disk: bool,
}

impl LiveStream {
    /// The disk read switch (ADR-0005): once every selected piece is in,
    /// the relay serves the file from disk at the same URL.
    pub(super) async fn switch_to_disk_when_complete(&mut self) {
        if self.on_disk
            || !self
                .session
                .stats(self.info_hash)
                .is_ok_and(|stats| stats.finished)
        {
            return;
        }
        if let Ok(relay) = self.streams.relay().await {
            self.on_disk = relay.mark_complete(&self.token).await;
        }
    }

    /// Session end (#179 Q6): revoke the token so the URL 404s and the
    /// relay drops its reader, then end the stream in the torrent module.
    pub(super) async fn end(self) {
        if let Ok(relay) = self.streams.relay().await {
            relay.revoke(&self.token).await;
        }
        self.streams
            .end_stream(self.info_hash, StreamOrigin::Playback)
            .await;
    }
}

/// The next Buffering flag change of `stream`, or never without one.
pub(super) async fn buffering_change(stream: Option<&mut LiveStream>) -> bool {
    let Some(stream) = stream else {
        return std::future::pending().await;
    };
    loop {
        match stream.buffering.recv().await {
            Ok(BufferingEvent::Stalled { token }) if token == stream.token => return true,
            Ok(BufferingEvent::Ready { token }) if token == stream.token => return false,
            Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
            Err(broadcast::error::RecvError::Closed) => return std::future::pending().await,
        }
    }
}

/// Whether an `Eof` from the player ends the episode. A torrent stream can
/// hit EOF on a failed read, so it counts only near the end (#179 Q4).
pub(super) fn eof_is_the_end(source: &StreamSource, state: &SessionState) -> bool {
    if !matches!(source, StreamSource::Torrent { .. }) {
        return true;
    }
    state
        .duration
        .is_some_and(|duration| state.position >= EOF_FRACTION * duration)
}

/// What `Preparing` hands to the launch.
struct Ready {
    session: Arc<TorrentSession>,
    file: DiskFile,
    reader: FileStream,
}

impl PlaybackManager {
    /// Reserves the slot and starts preparing `request` in the background, so
    /// a stop never waits on peers. Called with the command lock held.
    pub(super) fn start_torrent(
        self: &Arc<Self>,
        request: PlayRequest,
        info_hash: InfoHash,
        file_index: usize,
        resume: Option<db::Continuity>,
    ) -> Result<(), PlaybackError> {
        let streams = self
            .torrent
            .clone()
            .ok_or(PlaybackError::TorrentUnavailable)?;
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let (cancel, cancelled) = oneshot::channel();
        let (settle, settled) = oneshot::channel();
        *lock(&self.current) = Some(SessionControl::Preparing {
            id,
            cancel,
            settled,
        });
        streams.track(info_hash, StreamOrigin::Playback);
        let _ = self
            .events
            .send(AppEvent::PlaybackPreparing(Some(Preparing::new(
                request.media,
                request.episode,
            ))));
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager
                .run_torrent(
                    id,
                    streams,
                    request,
                    (info_hash, file_index),
                    resume,
                    (cancelled, settle),
                )
                .await;
        });
        Ok(())
    }

    /// Preparing → Running for one torrent request, then the session loop.
    async fn run_torrent(
        self: Arc<Self>,
        id: u64,
        streams: Arc<TorrentStreams>,
        request: PlayRequest,
        (info_hash, file_index): (InfoHash, usize),
        resume: Option<db::Continuity>,
        (mut cancelled, settle): (oneshot::Receiver<()>, oneshot::Sender<()>),
    ) {
        let prepared = tokio::select! {
            ready = self.prepare(&streams, &request, info_hash, file_index) => Some(ready),
            _ = &mut cancelled => None,
        };
        let _ = self.events.send(AppEvent::PlaybackPreparing(None));
        let launched = match prepared {
            Some(Ok(ready)) => {
                self.launch_stream(&streams, &request, info_hash, file_index, ready)
                    .await
            }
            Some(Err(error)) => Err(error),
            None => {
                debug!("torrent stream cancelled while preparing");
                streams.end_stream(info_hash, StreamOrigin::Playback).await;
                self.release_preparing(id, settle, None);
                return;
            }
        };
        let (session, stream) = match launched {
            Ok(launched) => launched,
            Err(error) => {
                warn!(%error, "the torrent stream could not start");
                streams.end_stream(info_hash, StreamOrigin::Playback).await;
                self.release_preparing(id, settle, Some((&request, error.to_string())));
                return;
            }
        };
        // A stop during the launch could not interrupt it; honour it now.
        if !matches!(
            cancelled.try_recv(),
            Err(oneshot::error::TryRecvError::Empty)
        ) {
            if let Err(error) = session.quit().await {
                debug!(%error, "player quit after a cancelled stream");
            }
            stream.end().await;
            self.release_preparing(id, settle, None);
            return;
        }
        let (stop, stopped) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        let (control, controls) = mpsc::unbounded_channel();
        *lock(&self.current) = Some(SessionControl::Running {
            id,
            stop,
            done: finished,
            control,
        });
        let _ = settle.send(());
        // Chapters come from the player's chapter-list only, and there are
        // no sidecar subtitles (#179 Q1).
        self.drive(
            id,
            session,
            request,
            resume,
            Vec::new(),
            stopped,
            controls,
            done,
            Some(stream),
        )
        .await;
    }

    /// Clears a preparing slot that never reached `Running`, tells the HUD
    /// and, on a failure, shows it as a retryable error.
    fn release_preparing(
        &self,
        id: u64,
        settle: oneshot::Sender<()>,
        failure: Option<(&PlayRequest, String)>,
    ) {
        {
            let mut current = lock(&self.current);
            if current.as_ref().is_some_and(|control| control.id() == id) {
                current.take();
            }
        }
        if let Some((request, message)) = failure {
            let _ = self.events.send(AppEvent::PlaybackError {
                media: request.media,
                episode: request.episode,
                message,
                retryable: true,
            });
        }
        let _ = self
            .events
            .send(AppEvent::PlaybackStopped { unexpected: false });
        let _ = settle.send(());
    }

    /// The `Preparing` phase: the torrent is added with only the episode's
    /// file and held until its readiness gate is met, with a report at 4 Hz.
    /// There is no timeout; dropping the future cancels it.
    async fn prepare(
        &self,
        streams: &TorrentStreams,
        request: &PlayRequest,
        info_hash: InfoHash,
        file_index: usize,
    ) -> Result<Ready, PlaybackError> {
        let mut report = Report::new(request, self.no_peers_after);
        let mut tick = tokio::time::interval(self.prepare_tick);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let session = streams.session().await?;
        let existing = session.stats(info_hash).is_ok();
        // The source carries identity only (#179 Q1); peers supply the rest.
        let magnet = TorrentSource::Magnet(format!("magnet:?xt=urn:btih:{info_hash}"));
        {
            let add = session.add(magnet, file_index);
            tokio::pin!(add);
            loop {
                tokio::select! {
                    added = &mut add => {
                        added?;
                        break;
                    }
                    _ = tick.tick() => self.publish_preparing(report.current()),
                }
            }
        }
        if existing {
            // A torrent kept seeding from an earlier stream, or a preload:
            // select this file, and undo a yield.
            session.update_only_files(info_hash, &[file_index]).await?;
            if session.stats(info_hash).is_ok_and(|stats| stats.paused) {
                session.resume(info_hash).await?;
            }
        }
        {
            let ready = session.wait_ready(info_hash, file_index);
            tokio::pin!(ready);
            loop {
                tokio::select! {
                    ready = &mut ready => {
                        ready?;
                        break;
                    }
                    _ = tick.tick() => {
                        self.publish_preparing(report.sample(&session, info_hash, file_index));
                    }
                }
            }
        }
        let file = session.file_on_disk(info_hash, file_index)?;
        let reader = session.stream(info_hash, file_index).await?;
        Ok(Ready {
            session,
            file,
            reader,
        })
    }

    /// Mints the stream's relay URL and opens the player on it.
    async fn launch_stream(
        &self,
        streams: &Arc<TorrentStreams>,
        request: &PlayRequest,
        info_hash: InfoHash,
        file_index: usize,
        ready: Ready,
    ) -> Result<(Box<dyn PlaybackSession>, LiveStream), PlaybackError> {
        let (stream, url) = mint_stream(streams, info_hash, file_index, ready).await?;
        let mut launch = self.player_launch(request, Vec::new());
        launch.stream_url = Some(url);
        match self.launcher.launch(launch).await {
            Ok(session) => Ok((session, stream)),
            Err(error) => {
                stream.end_token().await;
                Err(error.into())
            }
        }
    }

    /// Auto-next from the running stream to the torrent stream `next`
    /// (#179 Q5). Its torrent is held for playback first, then the old
    /// stream ends and a consumed preload lets go, so a batch torrent both
    /// use keeps running. `Preparing` shows until the gate is met.
    pub(super) async fn switch_stream(
        &self,
        session: &mut Box<dyn PlaybackSession>,
        stream: &mut Option<LiveStream>,
        stopped: &mut oneshot::Receiver<()>,
        next: &PlayRequest,
        info_hash: InfoHash,
        file_index: usize,
    ) -> Switch {
        let Some(streams) = self.torrent.clone() else {
            return Switch::Failed;
        };
        streams.track(info_hash, StreamOrigin::Playback);
        if let Some(old) = stream.take() {
            old.end().await;
        }
        let _ = self.events.send(AppEvent::PlaybackBuffering(false));
        self.set_stream_buffering(false);
        if let Some(fetch) = self.take_preload(next.media, next.episode) {
            self.release_preload_hold(fetch).await;
        }
        self.publish_preparing(Preparing::new(next.media, next.episode));
        let prepared = tokio::select! {
            ready = self.prepare(&streams, next, info_hash, file_index) => Some(ready),
            _ = &mut *stopped => None,
        };
        let _ = self.events.send(AppEvent::PlaybackPreparing(None));
        let ready = match prepared {
            Some(Ok(ready)) => ready,
            Some(Err(error)) => {
                warn!(%error, "the next torrent stream could not start");
                streams.end_stream(info_hash, StreamOrigin::Playback).await;
                return Switch::Failed;
            }
            None => {
                streams.end_stream(info_hash, StreamOrigin::Playback).await;
                return Switch::Stopped;
            }
        };
        let (live, url) = match mint_stream(&streams, info_hash, file_index, ready).await {
            Ok(minted) => minted,
            Err(error) => {
                warn!(%error, "the next torrent stream has no relay URL");
                streams.end_stream(info_hash, StreamOrigin::Playback).await;
                return Switch::Failed;
            }
        };
        // mpv's `loadfile` takes the URL where it takes a path.
        let opened = self
            .open_next(
                session,
                next,
                std::path::Path::new(&url),
                Vec::new(),
                Some(url.clone()),
            )
            .await;
        *stream = Some(live);
        if opened { Switch::Done } else { Switch::Failed }
    }

    fn publish_preparing(&self, preparing: Preparing) {
        let _ = self
            .events
            .send(AppEvent::PlaybackPreparing(Some(preparing)));
    }
}

/// Mints `ready`'s relay URL: the session's stream of `info_hash`.
async fn mint_stream(
    streams: &Arc<TorrentStreams>,
    info_hash: InfoHash,
    file_index: usize,
    ready: Ready,
) -> Result<(LiveStream, String), PlaybackError> {
    let relay = streams.relay().await?;
    let buffering = relay.subscribe_buffering();
    let spec = StreamSpec {
        info_hash,
        file_index,
        len: ready.file.len,
        disk_path: ready.file.path,
    };
    let (token, url) = relay.mint(spec, ready.reader).await;
    let stream = LiveStream {
        streams: Arc::clone(streams),
        session: ready.session,
        info_hash,
        token,
        buffering,
        on_disk: false,
    };
    Ok((stream, url))
}

impl LiveStream {
    /// Revokes the token only, for a launch that failed: the caller ends the
    /// stream itself.
    async fn end_token(self) {
        if let Ok(relay) = self.streams.relay().await {
            relay.revoke(&self.token).await;
        }
    }
}

/// The `Preparing` report being built, and when a peer was last seen.
struct Report {
    preparing: Preparing,
    last_peer: Instant,
    no_peers_after: Duration,
}

impl Report {
    fn new(request: &PlayRequest, no_peers_after: Duration) -> Self {
        Self {
            preparing: Preparing::new(request.media, request.episode),
            last_peer: Instant::now(),
            no_peers_after,
        }
    }

    /// The report as it stands, with the no-peers flag brought up to date.
    fn current(&mut self) -> Preparing {
        self.preparing.no_peers = self.last_peer.elapsed() >= self.no_peers_after;
        self.preparing
    }

    /// Peers, speed and window progress of the torrent being buffered.
    fn sample(
        &mut self,
        session: &TorrentSession,
        info_hash: InfoHash,
        file_index: usize,
    ) -> Preparing {
        if let Ok(stats) = session.stats(info_hash) {
            self.preparing.peers = stats.peers;
            self.preparing.download_bps = stats.download_bps;
        }
        if let Ok(readiness) = session.readiness(info_hash, file_index) {
            self.preparing.head = PieceProgress::new(readiness.head.done, readiness.head.total);
            self.preparing.tail = PieceProgress::new(readiness.tail.done, readiness.tail.total);
        }
        if self.preparing.peers > 0 {
            self.last_peer = Instant::now();
            self.preparing.stage = PrepareStage::Buffering;
        } else {
            self.preparing.stage = PrepareStage::Peers;
        }
        self.current()
    }
}

#[cfg(test)]
mod tests;
