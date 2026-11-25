//! The one-session playback manager: launch, continuity, watched and progress
//! sync (ticket #128, ROADMAP Phase 2, ARCHITECTURE §6).
//!
//! A play request stops the previous session, finds sidecar subtitles, reads
//! the saved position and opens a player. Resume is sent only when the player
//! reports `MediaReady`, never on the command line. Positions, pause changes
//! and the end of a session are published as [`AppEvent`]s for the HUD.

use std::{
    ffi::OsStr,
    fmt::Debug,
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use db::Db;
use domain::{
    AdvancedScores, AppEvent, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId,
    PlaybackControl, PlaybackDetails, PlaybackState, Score, StreamSource,
};
use flume::Sender;
use metadata::CatalogCache;
use player::{
    Error as PlayerError, InstalledPlayer, LaunchCommand, LaunchOptions, PlayerEvent, PlayerKind,
    SearchEnv, Session as MpvSession, detect, launch_iina, launch_mpchc, launch_mpv, launch_vlc,
    preferred,
};
use settings::{PlaybackKind, PlaybackSettings, PlayerChoice};
use thiserror::Error;
use tokio::sync::{mpsc, oneshot};
use tracing::{debug, warn};

use crate::BoxFuture;

/// How long a saved position is worth resuming.
const MIN_RESUME: f64 = 30.0;
/// The lowest and highest accepted watched threshold, in percent (OD-5).
const THRESHOLD_MIN: u8 = 50;
const THRESHOLD_MAX: u8 = 100;

/// One request to play an episode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlayRequest {
    /// The media the episode belongs to.
    pub media: MediaId,
    /// The episode's number in the main run.
    pub episode: u32,
    /// What the player should open.
    pub source: StreamSource,
    /// Labels for the HUD, supplied by the screen that requested play.
    pub details: Option<PlaybackDetails>,
}

impl PlayRequest {
    /// Plays `episode` of `media` from `source`, without HUD details.
    #[must_use]
    pub fn new(media: MediaId, episode: u32, source: StreamSource) -> Self {
        Self {
            media,
            episode,
            source,
            details: None,
        }
    }

    /// Attaches the labels the HUD shows while this episode plays.
    #[must_use]
    pub fn with_details(mut self, details: PlaybackDetails) -> Self {
        self.details = Some(details);
        self
    }
}

/// A progress-sync failure that the UI can show, with a Retry action when the
/// failure is likely to be transient.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct SyncError {
    message: String,
    retryable: bool,
}

impl SyncError {
    /// A failure a later attempt may fix, such as being offline.
    #[must_use]
    pub fn retryable(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    /// A failure a retry cannot fix, such as a read-only account.
    #[must_use]
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }

    /// The user-facing message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether the UI should offer Retry.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        self.retryable
    }
}

/// Writes watched progress to the active platform.
///
/// The composition root implements this over the platform gateway, so the
/// manager never names a platform crate's concrete type.
pub trait ProgressSync: Debug + Send + Sync + 'static {
    /// The active platform's entry for `media`, if it has one.
    fn entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, SyncError>>;

    /// Sets progress, creating a Watching entry when there is none.
    fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> BoxFuture<Result<ListEntry, SyncError>>;

    /// Saves a full entry, used to mark the final episode Completed.
    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, SyncError>>;
}

/// A sync over the account-free Local platform, used until the composition
/// root supplies the active platform's gateway.
#[derive(Debug, Clone)]
pub struct LocalProgressSync {
    db: Db,
}

impl LocalProgressSync {
    /// Syncs Local list entries in `db`.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

impl ProgressSync for LocalProgressSync {
    fn entry(&self, media: MediaId) -> BoxFuture<Result<Option<ListEntry>, SyncError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.local_list()
                .entry(media)
                .await
                .map_err(|error| SyncError::retryable(error.to_string()))
        })
    }

    fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> BoxFuture<Result<ListEntry, SyncError>> {
        let db = self.db.clone();
        Box::pin(async move {
            db.local_list()
                .set_progress(media, progress)
                .await
                .map_err(|error| SyncError::retryable(error.to_string()))
        })
    }

    fn save_entry(&self, entry: &ListEntry) -> BoxFuture<Result<ListEntry, SyncError>> {
        let db = self.db.clone();
        let entry = entry.clone();
        Box::pin(async move {
            db.local_list()
                .save(&entry)
                .await
                .map_err(|error| SyncError::retryable(error.to_string()))
        })
    }
}

/// A boxed session call, so [`PlaybackSession`] stays object-safe.
type SessionFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// The player end of one session.
///
/// Implemented for [`MpvSession`]; tests drive the manager with a fake event
/// stream through this seam.
pub trait PlaybackSession: Debug + Send + Sync {
    /// Waits for the next player event, or `None` when the player is gone.
    fn next_event(&mut self) -> SessionFuture<'_, Option<PlayerEvent>>;

    /// Seeks to an absolute position in seconds.
    fn seek(&self, seconds: f64) -> SessionFuture<'_, Result<(), PlayerError>>;

    /// Pauses or resumes the player.
    fn set_playback_state(
        &self,
        state: PlaybackControl,
    ) -> SessionFuture<'_, Result<(), PlayerError>>;

    /// Asks the player to quit.
    fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>>;
}

impl PlaybackSession for MpvSession {
    fn next_event(&mut self) -> SessionFuture<'_, Option<PlayerEvent>> {
        Box::pin(async move { MpvSession::next_event(self).await })
    }

    fn seek(&self, seconds: f64) -> SessionFuture<'_, Result<(), PlayerError>> {
        Box::pin(async move { MpvSession::seek(self, seconds).await })
    }

    fn set_playback_state(
        &self,
        state: PlaybackControl,
    ) -> SessionFuture<'_, Result<(), PlayerError>> {
        Box::pin(async move {
            if state == PlaybackControl::Pause {
                MpvSession::pause(self).await
            } else {
                MpvSession::resume(self).await
            }
        })
    }

    fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
        Box::pin(async move { MpvSession::quit(self).await })
    }
}

/// Everything needed to open one player.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct PlayerLaunch {
    /// The player selected in the Playback settings tab.
    pub choice: PlayerChoice,
    /// The source to open.
    pub source: StreamSource,
    /// Sidecar subtitles found next to the video.
    pub subtitles: Vec<PathBuf>,
    /// The app cache dir, where per-launch IPC sockets live.
    pub cache_dir: PathBuf,
    /// The media being played, for event routing.
    pub media: MediaId,
    /// The MPC-HC web-interface host.
    pub mpchc_host: String,
    /// The MPC-HC web-interface port.
    pub mpchc_port: u16,
}

/// Opens players. Injected so the manager can be tested without spawning one.
pub trait PlayerLauncher: Debug + Send + Sync + 'static {
    /// Launches a session for `launch`.
    fn launch(
        &self,
        launch: PlayerLaunch,
    ) -> BoxFuture<Result<Box<dyn PlaybackSession>, PlayerError>>;
}

/// Launches the player selected in the Playback tab through the `player` crate.
#[derive(Debug, Clone, Copy, Default)]
pub struct MpvLauncher;

/// The player kinds the Playback tab can name.
fn to_player_kind(kind: PlaybackKind) -> PlayerKind {
    match kind {
        PlaybackKind::Mpv => PlayerKind::Mpv,
        PlaybackKind::Iina => PlayerKind::Iina,
        PlaybackKind::Vlc => PlayerKind::Vlc,
        PlaybackKind::MpcHc => PlayerKind::MpcHc,
    }
}

impl MpvLauncher {
    /// Resolves the saved choice to an installed player and its command.
    async fn resolve(choice: &PlayerChoice) -> Result<(PlayerKind, PathBuf), PlayerError> {
        let env = SearchEnv::system().await;
        let installed = detect(&env).await;
        let selected = match choice {
            PlayerChoice::Auto => preferred(&installed, env.platform).cloned(),
            PlayerChoice::Installed { kind } => {
                let kind = to_player_kind(*kind);
                installed.iter().find(|player| player.kind == kind).cloned()
            }
            PlayerChoice::Custom { kind, path } => Some(InstalledPlayer {
                kind: to_player_kind(*kind),
                command: LaunchCommand::Executable(path.clone()),
                version: None,
            }),
        };
        let selected = selected.ok_or_else(|| {
            PlayerError::Endpoint("the selected player is not installed".to_owned())
        })?;
        let executable = match selected.command {
            LaunchCommand::Executable(path) => path,
            LaunchCommand::Flatpak(id) => {
                return Err(PlayerError::Endpoint(format!(
                    "Flatpak player {id} is not supported yet"
                )));
            }
            _ => {
                return Err(PlayerError::Endpoint(
                    "unsupported player launch command".to_owned(),
                ));
            }
        };
        Ok((selected.kind, executable))
    }
}

impl PlayerLauncher for MpvLauncher {
    fn launch(
        &self,
        launch: PlayerLaunch,
    ) -> BoxFuture<Result<Box<dyn PlaybackSession>, PlayerError>> {
        Box::pin(async move {
            let StreamSource::Local { path } = &launch.source else {
                return Err(PlayerError::Endpoint(
                    "only local files can be played in Phase 2".to_owned(),
                ));
            };
            let (kind, executable) = Self::resolve(&launch.choice).await?;
            let file = path.to_string_lossy().into_owned();
            let mut options = if kind == PlayerKind::Iina {
                LaunchOptions::iina(launch.cache_dir)
            } else {
                LaunchOptions::new(launch.cache_dir)
            };
            options.media = Some(launch.media);
            options.subtitles = launch.subtitles;
            let session = match kind {
                PlayerKind::Mpv => launch_mpv(&executable, &file, &options)?,
                PlayerKind::Iina => launch_iina(&executable, &file, &options)?,
                PlayerKind::Vlc => launch_vlc(&executable, &file, &options).await?,
                PlayerKind::MpcHc => {
                    launch_mpchc(
                        &executable,
                        &launch.mpchc_host,
                        launch.mpchc_port,
                        &file,
                        &options,
                    )
                    .await?
                }
                _ => {
                    return Err(PlayerError::Endpoint("unsupported player kind".to_owned()));
                }
            };
            let boxed: Box<dyn PlaybackSession> = Box::new(session);
            Ok(boxed)
        })
    }
}

/// Why a play request could not start.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum PlaybackError {
    /// The player could not be opened.
    #[error("could not open the player: {0}")]
    Player(#[from] PlayerError),
    /// The resume position could not be read.
    #[error("could not read continuity: {0}")]
    Db(#[from] db::DbError),
}

/// Sidecar subtitles with the video's stem, in its folder or `Subs/` (#128).
/// Results are sorted so a launch is reproducible.
#[must_use]
pub fn sidecar_subtitles(video: &Path) -> Vec<PathBuf> {
    const EXTENSIONS: [&str; 3] = ["ass", "ssa", "srt"];

    let Some(stem) = video.file_stem().and_then(OsStr::to_str) else {
        return Vec::new();
    };
    let Some(parent) = video.parent() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for dir in [parent.to_path_buf(), parent.join("Subs")] {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }
            let same_stem = path
                .file_stem()
                .and_then(OsStr::to_str)
                .is_some_and(|name| name.eq_ignore_ascii_case(stem));
            let subtitle = path.extension().and_then(OsStr::to_str).is_some_and(|ext| {
                EXTENSIONS
                    .iter()
                    .any(|wanted| ext.eq_ignore_ascii_case(wanted))
            });
            if same_stem && subtitle {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// A HUD command forwarded to the running session's event loop.
#[derive(Debug)]
enum Control {
    /// Play or pause.
    SetPlaybackState(PlaybackControl),
    /// Seek to an absolute position in seconds.
    Seek(f64),
}

/// The one slot a play request reserves before it awaits the launcher, so a
/// concurrent `stop()` waits for the launch instead of being lost.
#[derive(Debug)]
enum SessionControl {
    /// A launch is in flight; `settled` fires when it succeeds or fails.
    Launching {
        id: u64,
        settled: oneshot::Receiver<()>,
    },
    /// A running session; stopping it waits for its final save.
    Running {
        id: u64,
        stop: oneshot::Sender<()>,
        done: oneshot::Receiver<()>,
        control: mpsc::UnboundedSender<Control>,
    },
}

impl SessionControl {
    fn id(&self) -> u64 {
        match self {
            Self::Launching { id, .. } | Self::Running { id, .. } => *id,
        }
    }
}

#[derive(Debug)]
struct SessionState {
    resume: Option<db::Continuity>,
    duration: Option<f64>,
    position: f64,
    paused: bool,
    watched: bool,
    /// EOF was seen, so a following player exit is not an unexpected quit.
    eof: bool,
}

impl SessionState {
    fn new(resume: Option<db::Continuity>) -> Self {
        Self {
            resume,
            duration: None,
            position: 0.0,
            paused: false,
            watched: false,
            eof: false,
        }
    }
}

/// Drives the one active session and publishes its state to the UI.
#[derive(Debug)]
pub struct PlaybackManager {
    db: Db,
    catalog: CatalogCache,
    sync: Arc<Mutex<Arc<dyn ProgressSync>>>,
    events: Sender<AppEvent>,
    launcher: Arc<dyn PlayerLauncher>,
    cache_dir: PathBuf,
    player: PlayerChoice,
    mpchc_host: String,
    mpchc_port: u16,
    threshold: u8,
    resume: bool,
    auto_sync: bool,
    save_interval: Duration,
    next_id: AtomicU64,
    /// Serializes play and stop so a stop during a launch is never lost.
    commands: Arc<tokio::sync::Mutex<()>>,
    current: Arc<Mutex<Option<SessionControl>>>,
}

impl PlaybackManager {
    /// A manager that launches the selected player and saves every five seconds.
    #[must_use]
    pub fn new(
        db: Db,
        catalog: CatalogCache,
        sync: Arc<dyn ProgressSync>,
        events: Sender<AppEvent>,
        settings: &PlaybackSettings,
        cache_dir: PathBuf,
    ) -> Self {
        Self::with_launcher(
            db,
            catalog,
            sync,
            events,
            settings,
            cache_dir,
            Arc::new(MpvLauncher),
        )
    }

    /// A manager with an injected launcher, for tests that never spawn a player.
    #[must_use]
    pub fn with_launcher(
        db: Db,
        catalog: CatalogCache,
        sync: Arc<dyn ProgressSync>,
        events: Sender<AppEvent>,
        settings: &PlaybackSettings,
        cache_dir: PathBuf,
        launcher: Arc<dyn PlayerLauncher>,
    ) -> Self {
        Self {
            db,
            catalog,
            sync: Arc::new(Mutex::new(sync)),
            events,
            launcher,
            cache_dir,
            player: settings.player.clone(),
            mpchc_host: settings.mpchc_host.clone(),
            mpchc_port: settings.mpchc_port,
            threshold: settings
                .watched_threshold
                .clamp(THRESHOLD_MIN, THRESHOLD_MAX),
            resume: settings.resume,
            auto_sync: settings.auto_sync_progress,
            save_interval: Duration::from_secs(5),
            next_id: AtomicU64::new(1),
            commands: Arc::new(tokio::sync::Mutex::new(())),
            current: Arc::new(Mutex::new(None)),
        }
    }

    /// Replaces the progress sync, as when the active platform changes.
    pub fn set_sync(&self, sync: Arc<dyn ProgressSync>) {
        *lock(&self.sync) = sync;
    }

    /// Starts `request`, stopping any session already running.
    ///
    /// # Errors
    ///
    /// Returns an error when the previous position or the player cannot be
    /// opened. A failed launch leaves no session running.
    pub async fn play(self: &Arc<Self>, request: PlayRequest) -> Result<(), PlaybackError> {
        let _command = self.commands.lock().await;
        self.stop_inner().await;
        let StreamSource::Local { path } = &request.source else {
            return Err(PlaybackError::Player(PlayerError::Endpoint(
                "only local files can be played in Phase 2".to_owned(),
            )));
        };
        let resume = self
            .db
            .continuity()
            .get(request.media, request.episode)
            .await?;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (settle, settled) = oneshot::channel();
        *lock(&self.current) = Some(SessionControl::Launching { id, settled });
        let launch = PlayerLaunch {
            choice: self.player.clone(),
            source: request.source.clone(),
            subtitles: sidecar_subtitles(path),
            cache_dir: self.cache_dir.clone(),
            media: request.media,
            mpchc_host: self.mpchc_host.clone(),
            mpchc_port: self.mpchc_port,
        };
        let session = match self.launcher.launch(launch).await {
            Ok(session) => session,
            Err(error) => {
                let _ = settle.send(());
                lock(&self.current).take();
                return Err(error.into());
            }
        };
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
        let manager = Arc::clone(self);
        tokio::spawn(async move {
            manager
                .drive(id, session, request, resume, stopped, controls, done)
                .await;
        });
        Ok(())
    }

    /// Stops the active session and waits for its final position to be saved.
    pub async fn stop(&self) {
        let _command = self.commands.lock().await;
        self.stop_inner().await;
    }

    /// The body of [`Self::stop`], called with the command lock held.
    async fn stop_inner(&self) {
        let control = lock(&self.current).take();
        match control {
            Some(SessionControl::Launching { settled, .. }) => {
                let _ = settled.await;
            }
            Some(SessionControl::Running { stop, done, .. }) => {
                let _ = stop.send(());
                let _ = done.await;
            }
            None => {}
        }
    }

    /// Forwards one HUD command to the running session. Returns `false` when
    /// no session is running, so callers can report a stale command.
    fn send_control(&self, control: Control) -> bool {
        match lock(&self.current).as_ref() {
            Some(SessionControl::Running {
                control: sender, ..
            }) => sender.send(control).is_ok(),
            _ => false,
        }
    }

    /// Pauses or resumes the running player.
    pub fn set_playback_state(&self, state: PlaybackControl) -> bool {
        self.send_control(Control::SetPlaybackState(state))
    }

    /// Seeks the running player to `seconds`.
    pub fn seek(&self, seconds: f64) -> bool {
        self.send_control(Control::Seek(seconds))
    }

    /// Retries a failed progress sync for `episode`.
    pub async fn retry_sync(&self, media: MediaId, episode: u32) {
        self.sync_progress(media, episode).await;
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the session loop owns its player, stop signal, HUD commands and completion signal"
    )]
    async fn drive(
        self: &Arc<Self>,
        id: u64,
        mut session: Box<dyn PlaybackSession>,
        request: PlayRequest,
        resume: Option<db::Continuity>,
        mut stopped: oneshot::Receiver<()>,
        mut controls: mpsc::UnboundedReceiver<Control>,
        done: oneshot::Sender<()>,
    ) {
        let mut state = SessionState::new(resume);
        let mut tick = tokio::time::interval(self.save_interval);
        tick.tick().await; // The first tick is immediate; nothing to save yet.
        let mut unexpected = false;
        loop {
            enum Step {
                Event(Option<PlayerEvent>),
                Stop,
                Control(Control),
                Tick,
            }
            let step = tokio::select! {
                event = session.next_event() => Step::Event(event),
                result = &mut stopped => {
                    let _ = result;
                    Step::Stop
                }
                control = controls.recv() => match control {
                    Some(control) => Step::Control(control),
                    None => Step::Tick,
                },
                _ = tick.tick() => Step::Tick,
            };
            match step {
                Step::Event(None) => {
                    unexpected = !state.eof;
                    break;
                }
                Step::Stop => break,
                Step::Control(control) => {
                    let result = match control {
                        Control::SetPlaybackState(state) => {
                            session.set_playback_state(state).await
                        }
                        Control::Seek(seconds) => {
                            if seconds.is_finite() && seconds >= 0.0 {
                                state.position = seconds;
                                self.publish(&state, &request);
                            }
                            session.seek(seconds).await
                        }
                    };
                    if let Err(error) = result {
                        debug!(%error, "player rejected a HUD command");
                    }
                }
                Step::Event(Some(event)) => {
                    if matches!(event, PlayerEvent::Exited) {
                        unexpected = !state.eof;
                    }
                    if !self
                        .handle(session.as_ref(), &mut state, &request, event)
                        .await
                    {
                        break;
                    }
                }
                Step::Tick if !state.watched => self.save_position(&state, &request).await,
                Step::Tick => {}
            }
        }
        if state.watched {
            self.clear_position(&request).await;
        } else {
            self.save_position(&state, &request).await;
        }
        if let Err(error) = session.quit().await {
            debug!(%error, "player quit after the session ended");
        }
        let is_current = lock(&self.current)
            .as_ref()
            .is_some_and(|control| control.id() == id);
        if is_current {
            lock(&self.current).take();
            let _ = self.events.send(AppEvent::PlaybackStopped { unexpected });
        }
        let _ = done.send(());
    }

    /// Returns `false` when the session is over.
    async fn handle(
        &self,
        session: &dyn PlaybackSession,
        state: &mut SessionState,
        request: &PlayRequest,
        event: PlayerEvent,
    ) -> bool {
        match event {
            PlayerEvent::MediaReady { duration, .. } => {
                state.duration = duration.filter(|value| value.is_finite() && *value > 0.0);
                // Without a duration we cannot tell whether the saved position
                // is already watched, so wait for the duration event.
                if state.duration.is_some() {
                    self.apply_resume(session, state).await;
                }
                self.publish(state, request);
            }
            PlayerEvent::Duration { seconds } if seconds.is_finite() && seconds > 0.0 => {
                state.duration = Some(seconds);
                self.apply_resume(session, state).await;
            }
            PlayerEvent::Position { seconds } => {
                if seconds.is_finite() && seconds >= 0.0 {
                    state.position = seconds;
                    self.publish(state, request);
                    if self.is_watched(state) {
                        self.mark_watched(state, request).await;
                    }
                }
            }
            PlayerEvent::Paused(paused) => {
                state.paused = paused;
                self.publish(state, request);
            }
            PlayerEvent::Eof => {
                state.eof = true;
                self.mark_watched(state, request).await;
            }
            PlayerEvent::Exited => return false,
            _ => {}
        }
        true
    }

    /// Seeks to the saved position once the duration is known, unless the
    /// position is too short or already past the watched threshold.
    async fn apply_resume(&self, session: &dyn PlaybackSession, state: &mut SessionState) {
        if !self.resume {
            return;
        }
        let Some(resume) = state.resume.take() else {
            return;
        };
        let past_threshold = state
            .duration
            .is_some_and(|duration| resume.position >= self.threshold_fraction() * duration);
        if resume.position >= MIN_RESUME
            && !past_threshold
            && let Err(error) = session.seek(resume.position).await
        {
            warn!(%error, "could not resume the saved position");
        }
    }

    fn is_watched(&self, state: &SessionState) -> bool {
        state
            .duration
            .is_some_and(|duration| state.position >= self.threshold_fraction() * duration)
    }

    fn threshold_fraction(&self) -> f64 {
        f64::from(self.threshold) / 100.0
    }

    fn publish(&self, state: &SessionState, request: &PlayRequest) {
        let update = PlaybackState {
            media: request.media,
            episode: request.episode,
            position: seconds(state.position),
            duration: state.duration.map(seconds),
            paused: state.paused,
        };
        let _ = self.events.send(AppEvent::PlaybackState(update));
    }

    async fn mark_watched(&self, state: &mut SessionState, request: &PlayRequest) {
        if state.watched {
            return;
        }
        state.watched = true;
        self.clear_position(request).await;
        if self.auto_sync {
            self.sync_progress(request.media, request.episode).await;
        }
    }

    async fn save_position(&self, state: &SessionState, request: &PlayRequest) {
        let result = self
            .db
            .continuity()
            .save(
                request.media,
                request.episode,
                state.position,
                state.duration,
            )
            .await;
        if let Err(error) = result {
            warn!(%error, "could not save a resume position");
        }
    }

    async fn clear_position(&self, request: &PlayRequest) {
        let result = self
            .db
            .continuity()
            .clear(request.media, request.episode)
            .await;
        if let Err(error) = result {
            warn!(%error, "could not clear a watched episode's position");
        }
    }

    /// Applies OD-5: forward only, Watching when absent, Completed on the last
    /// episode, and a retryable event when the platform refuses the write.
    async fn sync_progress(&self, media: MediaId, episode: u32) {
        let total = self.episode_count(media).await;
        self.sync_progress_with(media, episode, total).await;
    }

    /// The sync policy with the episode count already resolved, so a test can
    /// drive the last-episode rule without populating the catalog cache.
    async fn sync_progress_with(&self, media: MediaId, episode: u32, total: Option<u32>) {
        let sync = Arc::clone(&lock(&self.sync));
        let current = match sync.entry(media).await {
            Ok(entry) => entry,
            Err(error) => {
                self.report_sync_error(media, episode, &error);
                return;
            }
        };
        let progress = current
            .as_ref()
            .map_or(episode, |entry| entry.progress.max(episode));
        let completed = total.is_some_and(|count| progress >= count);
        if current.as_ref().is_some_and(|entry| {
            entry.progress == progress && (!completed || entry.status == ListStatus::Completed)
        }) {
            return;
        }
        let result = match (completed, current) {
            (true, Some(mut entry)) => {
                entry.status = ListStatus::Completed;
                entry.progress = progress;
                sync.save_entry(&entry).await.map(|_| ())
            }
            (true, None) => {
                let entry = new_entry(media, progress, ListStatus::Completed);
                sync.save_entry(&entry).await.map(|_| ())
            }
            (false, _) => sync.update_progress(media, progress).await.map(|_| ()),
        };
        if let Err(error) = result {
            self.report_sync_error(media, episode, &error);
        }
    }

    fn report_sync_error(&self, media: MediaId, episode: u32, error: &SyncError) {
        let _ = self.events.send(AppEvent::PlaybackError {
            media,
            episode,
            message: error.message().to_owned(),
            retryable: error.is_retryable(),
        });
    }

    /// The AniList episode count from the cache only: sync never fetches.
    async fn episode_count(&self, media: MediaId) -> Option<u32> {
        let ids = self.db.media().external_ids(media).await.ok()?;
        let id = ids.into_iter().find_map(|id| match id {
            ExternalId::AniList(id) => Some(id),
            _ => None,
        })?;
        if let Some(cached) = self.catalog.cached_media(id).await {
            return cached.value.summary.episodes;
        }
        // A media the Entry page has never opened has no cached count. Fetch
        // once at watched time; offline loads still return their cached value.
        self.catalog
            .media(id)
            .await
            .ok()
            .and_then(|load| load.cached.value.summary.episodes)
    }
}

fn new_entry(media: MediaId, progress: u32, status: ListStatus) -> ListEntry {
    ListEntry {
        media,
        status,
        score: Score::UNSCORED,
        progress,
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
        created_at: SystemTime::UNIX_EPOCH,
        updated_at: SystemTime::UNIX_EPOCH,
    }
}

fn seconds(value: f64) -> Duration {
    if value.is_finite() && value > 0.0 {
        Duration::from_secs_f64(value)
    } else {
        Duration::ZERO
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use domain::{ExternalId, ListStatus, MediaKind};
    use futures::{SinkExt, StreamExt, channel::mpsc};
    use settings::PlaybackSettings;
    use tokio::sync::Mutex as AsyncMutex;

    use super::*;

    fn entry(media: MediaId, progress: u32, status: ListStatus) -> ListEntry {
        new_entry(media, progress, status)
    }

    #[derive(Debug, Default)]
    struct FakeSync {
        entry: Arc<Mutex<Option<ListEntry>>>,
        updates: Arc<Mutex<Vec<u32>>>,
        saves: Arc<Mutex<Vec<ListEntry>>>,
        failure: Arc<Mutex<Option<SyncError>>>,
    }

    impl FakeSync {
        fn set_entry(&self, value: Option<ListEntry>) {
            *lock(&self.entry) = value;
        }

        fn set_failure(&self, error: SyncError) {
            *lock(&self.failure) = Some(error);
        }

        fn updates(&self) -> Vec<u32> {
            lock(&self.updates).clone()
        }

        fn saves(&self) -> Vec<ListEntry> {
            lock(&self.saves).clone()
        }

        fn fail(&self) -> Option<SyncError> {
            lock(&self.failure).clone()
        }
    }

    impl ProgressSync for FakeSync {
        fn entry(&self, _media: MediaId) -> BoxFuture<Result<Option<ListEntry>, SyncError>> {
            let (entry, failure) = (lock(&self.entry).clone(), self.fail());
            Box::pin(async move { failure.map_or(Ok(entry), Err) })
        }

        fn update_progress(
            &self,
            media: MediaId,
            progress: u32,
        ) -> BoxFuture<Result<ListEntry, SyncError>> {
            lock(&self.updates).push(progress);
            let mut slot = lock(&self.entry);
            let saved = slot.get_or_insert_with(|| entry(media, 0, ListStatus::Current));
            saved.progress = progress;
            let saved = saved.clone();
            let failure = self.fail();
            Box::pin(async move { failure.map_or(Ok(saved), Err) })
        }

        fn save_entry(&self, value: &ListEntry) -> BoxFuture<Result<ListEntry, SyncError>> {
            lock(&self.saves).push(value.clone());
            *lock(&self.entry) = Some(value.clone());
            let failure = self.fail();
            let saved = value.clone();
            Box::pin(async move { failure.map_or(Ok(saved), Err) })
        }
    }

    #[derive(Debug, Default, Clone)]
    struct FakeLauncher {
        seeks: Arc<Mutex<Vec<f64>>>,
        pauses: Arc<Mutex<Vec<PlaybackControl>>>,
        quits: Arc<AtomicUsize>,
        launches: Arc<AtomicUsize>,
        gate: Arc<Mutex<Option<oneshot::Receiver<()>>>>,
        /// Keeps each session's event sender alive until its test ends.
        senders: Arc<Mutex<Vec<mpsc::UnboundedSender<PlayerEvent>>>>,
    }

    impl FakeLauncher {
        fn session(&self) -> (Box<dyn PlaybackSession>, mpsc::UnboundedSender<PlayerEvent>) {
            let (tx, rx) = mpsc::unbounded();
            lock(&self.senders).push(tx.clone());
            let session = FakeSession {
                rx: AsyncMutex::new(rx),
                seeks: Arc::clone(&self.seeks),
                pauses: Arc::clone(&self.pauses),
                quits: Arc::clone(&self.quits),
            };
            (Box::new(session), tx)
        }

        fn seeks(&self) -> Vec<f64> {
            lock(&self.seeks).clone()
        }

        fn pauses(&self) -> Vec<PlaybackControl> {
            lock(&self.pauses).clone()
        }

        fn set_gate(&self, gate: oneshot::Receiver<()>) {
            *lock(&self.gate) = Some(gate);
        }

        async fn wait_for_launch(&self) {
            while self.launches.load(Ordering::SeqCst) == 0 {
                tokio::task::yield_now().await;
            }
        }
    }

    impl PlayerLauncher for FakeLauncher {
        fn launch(
            &self,
            _launch: PlayerLaunch,
        ) -> BoxFuture<Result<Box<dyn PlaybackSession>, PlayerError>> {
            self.launches.fetch_add(1, Ordering::SeqCst);
            let (session, _) = self.session();
            let gate = lock(&self.gate).take();
            Box::pin(async move {
                if let Some(gate) = gate {
                    let _ = gate.await;
                }
                Ok(session)
            })
        }
    }

    #[derive(Debug)]
    struct FakeSession {
        rx: AsyncMutex<mpsc::UnboundedReceiver<PlayerEvent>>,
        seeks: Arc<Mutex<Vec<f64>>>,
        pauses: Arc<Mutex<Vec<PlaybackControl>>>,
        quits: Arc<AtomicUsize>,
    }

    impl PlaybackSession for FakeSession {
        fn next_event(&mut self) -> SessionFuture<'_, Option<PlayerEvent>> {
            Box::pin(async move { self.rx.lock().await.next().await })
        }

        fn seek(&self, seconds: f64) -> SessionFuture<'_, Result<(), PlayerError>> {
            let seeks = Arc::clone(&self.seeks);
            Box::pin(async move {
                lock(&seeks).push(seconds);
                Ok(())
            })
        }

        fn set_playback_state(
            &self,
            state: PlaybackControl,
        ) -> SessionFuture<'_, Result<(), PlayerError>> {
            let seen = Arc::clone(&self.pauses);
            Box::pin(async move {
                lock(&seen).push(state);
                Ok(())
            })
        }

        fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
            let quits = Arc::clone(&self.quits);
            Box::pin(async move {
                quits.fetch_add(1, Ordering::SeqCst);
                Ok(())
            })
        }
    }

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Db,
        manager: Arc<PlaybackManager>,
        sync: Arc<FakeSync>,
        launcher: FakeLauncher,
        events: flume::Receiver<AppEvent>,
    }

    impl Fixture {
        async fn new(settings: PlaybackSettings) -> Self {
            Self::with_interval(settings, Duration::from_secs(5)).await
        }

        async fn with_interval(settings: PlaybackSettings, interval: Duration) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
            let cache = dir.path().join("cache");
            fs::create_dir_all(&cache).unwrap();
            let client = net::Client::new().unwrap();
            let poster = net::PosterCache::new(&cache).unwrap();
            let catalog = CatalogCache::new(metadata::AniList::new(client), poster);
            let (events, receiver) = flume::unbounded();
            let sync = Arc::new(FakeSync::default());
            let launcher = FakeLauncher::default();
            let progress: Arc<dyn ProgressSync> = Arc::<FakeSync>::clone(&sync);
            let mut manager = PlaybackManager::with_launcher(
                db.clone(),
                catalog,
                progress,
                events,
                &settings,
                cache,
                Arc::new(launcher.clone()),
            );
            manager.save_interval = interval;
            let manager = Arc::new(manager);
            Self {
                _dir: dir,
                db,
                manager,
                sync,
                launcher,
                events: receiver,
            }
        }

        async fn media(&self, id: u32) -> MediaId {
            self.db
                .media()
                .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(id)])
                .await
                .unwrap()
                .media
        }

        fn drive(
            &self,
            resume: Option<db::Continuity>,
            request: &PlayRequest,
        ) -> (
            mpsc::UnboundedSender<PlayerEvent>,
            oneshot::Sender<()>,
            oneshot::Receiver<()>,
        ) {
            let (session, events) = self.launcher.session();
            let (stop, stopped) = oneshot::channel();
            let (done, finished) = oneshot::channel();
            let (_control, controls) = tokio::sync::mpsc::unbounded_channel();
            let manager = Arc::clone(&self.manager);
            let request = request.clone();
            tokio::spawn(async move {
                manager
                    .drive(1, session, request, resume, stopped, controls, done)
                    .await;
            });
            (events, stop, finished)
        }

        fn states(&self) -> Vec<PlaybackState> {
            self.events
                .try_iter()
                .filter_map(|event| match event {
                    AppEvent::PlaybackState(state) => Some(state),
                    _ => None,
                })
                .collect()
        }

        fn errors(&self) -> Vec<(bool, String)> {
            self.events
                .try_iter()
                .filter_map(|event| match event {
                    AppEvent::PlaybackError {
                        retryable, message, ..
                    } => Some((retryable, message)),
                    _ => None,
                })
                .collect()
        }
    }

    fn request(media: MediaId, episode: u32) -> PlayRequest {
        PlayRequest::new(media, episode, StreamSource::local("/media/ep.mkv"))
    }

    async fn finish(
        mut events: mpsc::UnboundedSender<PlayerEvent>,
        _stop: oneshot::Sender<()>,
        done: oneshot::Receiver<()>,
    ) {
        events
            .send(PlayerEvent::Exited)
            .await
            .expect("the fake player accepts the event");
        done.await.expect("the session finished");
    }

    #[tokio::test]
    async fn resume_is_sent_only_after_media_ready() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let resume = Some(db::Continuity {
            position: 40.0,
            duration: Some(100.0),
        });
        let (mut events, stop, done) = fixture.drive(resume, &request);

        events
            .send(PlayerEvent::Position { seconds: 5.0 })
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert!(fixture.launcher.seeks().is_empty(), "no early seek");

        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert_eq!(fixture.launcher.seeks(), [40.0]);
    }

    #[tokio::test]
    async fn a_position_under_thirty_seconds_does_not_resume() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (mut events, stop, done) = fixture.drive(
            Some(db::Continuity {
                position: 20.0,
                duration: Some(100.0),
            }),
            &request,
        );
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert!(
            fixture.launcher.seeks().is_empty(),
            "20 s is not worth resuming"
        );
    }

    #[tokio::test]
    async fn a_position_past_the_threshold_does_not_resume() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (mut events, stop, done) = fixture.drive(
            Some(db::Continuity {
                position: 90.0,
                duration: Some(100.0),
            }),
            &request,
        );
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert!(
            fixture.launcher.seeks().is_empty(),
            "90% is already watched"
        );
    }

    #[tokio::test]
    async fn a_late_duration_event_still_enables_resume() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (mut events, stop, done) = fixture.drive(
            Some(db::Continuity {
                position: 40.0,
                duration: Some(100.0),
            }),
            &request(media, 1),
        );
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: None,
            })
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert!(
            fixture.launcher.seeks().is_empty(),
            "duration is not known yet"
        );

        events
            .send(PlayerEvent::Duration { seconds: 100.0 })
            .await
            .unwrap();
        finish(events, stop, done).await;
        assert_eq!(fixture.launcher.seeks(), [40.0]);
    }

    #[tokio::test]
    async fn a_late_duration_event_enables_watched_at_the_threshold() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (mut events, stop, done) = fixture.drive(None, &request(media, 1));
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: None,
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Duration { seconds: 100.0 })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 80.0 })
            .await
            .unwrap();
        finish(events, stop, done).await;
        assert_eq!(fixture.sync.updates(), [1]);
    }

    #[tokio::test]
    async fn a_stop_during_a_launch_stops_the_session_that_opens() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (release, gate) = oneshot::channel();
        fixture.launcher.set_gate(gate);

        let manager = Arc::clone(&fixture.manager);
        let play = tokio::spawn(async move { manager.play(request(media, 1)).await });
        tokio::time::timeout(Duration::from_secs(2), fixture.launcher.wait_for_launch())
            .await
            .expect("the launcher was called");
        let manager = Arc::clone(&fixture.manager);
        let stop = tokio::spawn(async move { manager.stop().await });
        tokio::task::yield_now().await;
        let _ = release.send(());

        play.await.unwrap().unwrap();
        stop.await.unwrap();
        assert_eq!(
            fixture.launcher.quits.load(Ordering::SeqCst),
            1,
            "the player opened during stop() was quit"
        );
    }

    #[tokio::test]
    async fn overlapping_plays_leave_one_session() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (release, gate) = oneshot::channel();
        fixture.launcher.set_gate(gate);

        let first = Arc::clone(&fixture.manager);
        let play = tokio::spawn(async move { first.play(request(media, 1)).await });
        tokio::time::timeout(Duration::from_secs(2), fixture.launcher.wait_for_launch())
            .await
            .expect("the launcher was called");
        let second = Arc::clone(&fixture.manager);
        let next = tokio::spawn(async move { second.play(request(media, 2)).await });
        let _ = release.send(());

        play.await.unwrap().unwrap();
        next.await.unwrap().unwrap();
        assert_eq!(fixture.launcher.launches.load(Ordering::SeqCst), 2);
        assert_eq!(
            fixture.launcher.quits.load(Ordering::SeqCst),
            1,
            "the superseded session was stopped"
        );
    }

    #[tokio::test]
    async fn watched_fires_once_at_the_threshold_and_at_eof() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (mut events, stop, done) = fixture.drive(None, &request);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 80.0 })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 85.0 })
            .await
            .unwrap();
        events.send(PlayerEvent::Eof).await.unwrap();
        finish(events, stop, done).await;

        assert_eq!(fixture.sync.updates(), [1], "watched is once per session");
        assert_eq!(
            fixture.db.continuity().get(media, 1).await.unwrap(),
            None,
            "a watched episode has no resume row"
        );
        assert!(
            fixture
                .states()
                .iter()
                .any(|state| state.position == Duration::from_secs(80)),
            "the HUD sees the position"
        );
    }

    #[tokio::test]
    async fn eof_watches_before_the_threshold() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (mut events, stop, done) = fixture.drive(None, &request);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(600.0),
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 30.0 })
            .await
            .unwrap();
        events.send(PlayerEvent::Eof).await.unwrap();
        finish(events, stop, done).await;

        assert_eq!(fixture.sync.updates(), [1], "EOF always watches");
    }

    #[tokio::test]
    async fn auto_sync_off_keeps_the_position_but_does_not_write_progress() {
        let settings = PlaybackSettings {
            auto_sync_progress: false,
            ..PlaybackSettings::default()
        };
        let fixture = Fixture::new(settings).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (mut events, stop, done) = fixture.drive(None, &request);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        events.send(PlayerEvent::Eof).await.unwrap();
        finish(events, stop, done).await;

        assert!(fixture.sync.updates().is_empty(), "auto-sync was off");
        assert_eq!(fixture.db.continuity().get(media, 1).await.unwrap(), None);
    }

    #[tokio::test]
    async fn progress_never_moves_backwards() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture
            .sync
            .set_entry(Some(entry(media, 12, ListStatus::Current)));

        fixture.manager.sync_progress_with(media, 5, Some(24)).await;

        assert!(fixture.sync.updates().is_empty(), "no backwards write");
        assert_eq!(fixture.sync.saves(), Vec::new());
    }

    #[tokio::test]
    async fn a_media_without_an_entry_becomes_watching() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;

        fixture.manager.sync_progress_with(media, 3, None).await;

        assert_eq!(fixture.sync.updates(), [3]);
        let saved = lock(&fixture.sync.entry).clone().unwrap();
        assert_eq!(saved.status, ListStatus::Current, "Watching");
        assert_eq!(saved.progress, 3);
    }

    #[tokio::test]
    async fn the_final_episode_sets_completed() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture
            .sync
            .set_entry(Some(entry(media, 11, ListStatus::Current)));

        fixture
            .manager
            .sync_progress_with(media, 12, Some(12))
            .await;

        let saved = fixture.sync.saves();
        assert_eq!(saved.len(), 1, "the status must be saved in full");
        let saved = saved.first().expect("one save");
        assert_eq!(saved.status, ListStatus::Completed);
        assert_eq!(saved.progress, 12);
    }

    #[tokio::test]
    async fn a_platform_failure_publishes_a_retryable_error_event() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture.sync.set_failure(SyncError::retryable("offline"));

        fixture.manager.sync_progress_with(media, 4, None).await;

        assert_eq!(fixture.errors(), [(true, "offline".to_owned())]);
    }

    #[tokio::test]
    async fn a_read_only_account_error_still_offers_retry() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture
            .sync
            .set_failure(SyncError::retryable("log in again"));

        fixture.manager.sync_progress_with(media, 4, None).await;

        assert_eq!(fixture.errors(), [(true, "log in again".to_owned())]);
    }

    #[tokio::test]
    async fn positions_are_saved_every_interval_and_on_stop() {
        let fixture =
            Fixture::with_interval(PlaybackSettings::default(), Duration::from_millis(20)).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (mut events, stop, done) = fixture.drive(None, &request);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 42.0 })
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(80)).await;
        let saved = fixture
            .db
            .continuity()
            .get(media, 1)
            .await
            .unwrap()
            .expect("a periodic save");
        assert_eq!(saved.position, 42.0);

        events
            .send(PlayerEvent::Position { seconds: 55.0 })
            .await
            .unwrap();
        events.send(PlayerEvent::Exited).await.unwrap();
        done.await.unwrap();
        drop(stop);
        let final_save = fixture
            .db
            .continuity()
            .get(media, 1)
            .await
            .unwrap()
            .expect("a save on stop");
        assert_eq!(final_save.position, 55.0);
    }

    #[tokio::test]
    async fn hud_commands_reach_the_running_session() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture.manager.play(request(media, 1)).await.unwrap();
        tokio::time::timeout(Duration::from_secs(2), fixture.launcher.wait_for_launch())
            .await
            .expect("the launcher was called");

        assert!(
            fixture
                .manager
                .set_playback_state(PlaybackControl::Pause),
            "a session is running"
        );
        assert!(fixture.manager.seek(42.0));
        tokio::time::timeout(Duration::from_secs(2), async {
            while fixture.launcher.pauses().is_empty() || fixture.launcher.seeks().is_empty() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the session handled both commands");

        assert_eq!(fixture.launcher.pauses(), [PlaybackControl::Pause]);
        assert_eq!(fixture.launcher.seeks(), [42.0]);
        fixture.manager.stop().await;
    }

    #[tokio::test]
    async fn a_new_play_stops_the_old_session_first() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture.manager.play(request(media, 1)).await.unwrap();
        let first = fixture.launcher.quits.load(Ordering::SeqCst);
        fixture.manager.play(request(media, 2)).await.unwrap();
        assert!(
            fixture.launcher.quits.load(Ordering::SeqCst) > first,
            "the old player was asked to quit before the new one launched"
        );
        fixture.manager.stop().await;
    }

    #[test]
    fn sidecar_subtitles_come_from_the_video_folder_and_subs() {
        let dir = tempfile::tempdir().unwrap();
        let video = dir.path().join("Show - 01.mkv");
        fs::write(&video, b"video").unwrap();
        fs::write(dir.path().join("Show - 01.ass"), b"sub").unwrap();
        fs::write(dir.path().join("other.srt"), b"sub").unwrap();
        fs::create_dir(dir.path().join("Subs")).unwrap();
        fs::write(dir.path().join("Subs/Show - 01.SRT"), b"sub").unwrap();
        fs::write(dir.path().join("Subs/Show - 02.ass"), b"sub").unwrap();

        assert_eq!(
            sidecar_subtitles(&video),
            vec![
                dir.path().join("Show - 01.ass"),
                dir.path().join("Subs/Show - 01.SRT"),
            ]
        );
    }

    #[derive(Debug)]
    struct RecordingSession {
        inner: MpvSession,
        seeks: Arc<Mutex<Vec<f64>>>,
    }

    impl PlaybackSession for RecordingSession {
        fn next_event(&mut self) -> SessionFuture<'_, Option<PlayerEvent>> {
            Box::pin(async move { self.inner.next_event().await })
        }

        fn seek(&self, seconds: f64) -> SessionFuture<'_, Result<(), PlayerError>> {
            let seeks = Arc::clone(&self.seeks);
            Box::pin(async move {
                lock(&seeks).push(seconds);
                self.inner.seek(seconds).await
            })
        }

        fn set_playback_state(
            &self,
            state: PlaybackControl,
        ) -> SessionFuture<'_, Result<(), PlayerError>> {
            Box::pin(async move {
                if state == PlaybackControl::Pause {
                    self.inner.pause().await
                } else {
                    self.inner.resume().await
                }
            })
        }

        fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
            Box::pin(async move { self.inner.quit().await })
        }
    }

    #[derive(Debug)]
    struct RealMpvLauncher {
        executable: String,
        cache: PathBuf,
        seeks: Arc<Mutex<Vec<f64>>>,
    }

    impl PlayerLauncher for RealMpvLauncher {
        fn launch(
            &self,
            launch: PlayerLaunch,
        ) -> BoxFuture<Result<Box<dyn PlaybackSession>, PlayerError>> {
            let StreamSource::Local { path } = &launch.source else {
                return Box::pin(async {
                    Err(PlayerError::Endpoint("not a local file".to_owned()))
                });
            };
            let mut options = LaunchOptions::new(self.cache.clone());
            options.media = Some(launch.media);
            options.subtitles = launch.subtitles.clone();
            let inner = match launch_mpv(&self.executable, &path.to_string_lossy(), &options) {
                Ok(inner) => inner,
                Err(error) => return Box::pin(async move { Err(error) }),
            };
            let session = RecordingSession {
                inner,
                seeks: Arc::clone(&self.seeks),
            };
            Box::pin(async move {
                let boxed: Box<dyn PlaybackSession> = Box::new(session);
                Ok(boxed)
            })
        }
    }

    /// Manual check for the Phase 2 exit: `AUTUMN_TEST_MPV=/path/to/mpv
    /// AUTUMN_TEST_VIDEO=/path/to/clip.mkv cargo test -p engine resume_with_real_mpv`.
    #[tokio::test]
    #[ignore = "needs a real mpv and a video file; see the doc comment"]
    async fn resume_with_real_mpv() {
        let (Ok(executable), Ok(video)) = (
            std::env::var("AUTUMN_TEST_MPV"),
            std::env::var("AUTUMN_TEST_VIDEO"),
        ) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        let cache = dir.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let catalog = CatalogCache::new(
            metadata::AniList::new(net::Client::new().unwrap()),
            net::PosterCache::new(&cache).unwrap(),
        );
        let (sender, receiver) = flume::unbounded();
        let sync: Arc<dyn ProgressSync> = Arc::new(LocalProgressSync::new(db.clone()));
        let seeks = Arc::new(Mutex::new(Vec::new()));
        let launcher = RealMpvLauncher {
            executable,
            cache: cache.clone(),
            seeks: Arc::clone(&seeks),
        };
        let settings = PlaybackSettings::default();
        let manager = Arc::new(PlaybackManager::with_launcher(
            db.clone(),
            catalog,
            sync,
            sender,
            &settings,
            cache,
            Arc::new(launcher),
        ));
        let media = db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
            .await
            .unwrap()
            .media;
        let request = PlayRequest::new(media, 1, StreamSource::local(video));

        manager.play(request.clone()).await.unwrap();
        let saved = tokio::time::timeout(Duration::from_secs(90), async {
            loop {
                if let Ok(AppEvent::PlaybackState(state)) = receiver.recv_async().await
                    && state.position >= Duration::from_secs(40)
                {
                    return state.position;
                }
            }
        })
        .await
        .expect("mpv reached 40 seconds");
        manager.stop().await;
        let stored = db.continuity().get(media, 1).await.unwrap().unwrap();
        assert!(stored.position >= 40.0, "continuity kept the 40 s position");

        manager.play(request).await.unwrap();
        tokio::time::timeout(Duration::from_secs(90), async {
            while lock(&seeks).is_empty() {
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
        })
        .await
        .expect("the second session resumed");
        let resume = lock(&seeks).first().copied().unwrap_or(0.0);
        assert!(
            (resume - saved.as_secs_f64()).abs() < 5.0,
            "mpv resumed near the saved position: {resume}"
        );
        manager.stop().await;
    }
}
