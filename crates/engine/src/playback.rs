//! The one-session playback manager: launch, continuity, watched and progress
//! sync (ticket #128, ROADMAP Phase 2, ARCHITECTURE §6).
//!
//! A play request stops the previous session, finds sidecar subtitles, reads
//! the saved position and opens a player. Resume is sent only when the player
//! reports `MediaReady`, never on the command line. Positions, pause changes
//! and the end of a session are published as [`AppEvent`]s for the HUD.
//!
//! When an episode ends and the library has the next one, a countdown runs
//! inside the same session loop (#131). At zero the next file replaces the
//! current one in the same player, or in a fresh launch for players that
//! cannot replace a file.
//!
//! A torrent source first goes through the `Preparing` phase (#217, #179):
//! see the `stream` submodule. Halfway through a torrent stream the next
//! episode is preloaded (#224, #208): see the `preload` submodule. Auto-next
//! for a torrent stream tries the library, then the same batch torrent,
//! then the preload (#179 Q5).

use std::{
    ffi::OsStr,
    fmt::Debug,
    fs,
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, SystemTime},
};

use db::Db;
use domain::{
    AdvancedScores, AppEvent, AutoNext, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId,
    PlaybackChapter, PlaybackControl, PlaybackDetails, PlaybackState, Score, StreamSource,
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

use crate::{AutoSelectRequest, AutoSelector, BoxFuture, TorrentStreams, chapters, library};

mod preload;
mod stream;

pub use preload::{AniZipAiring, EpisodeAiring, Preloaded};
use stream::LiveStream;

/// How long a saved position is worth resuming. Home uses the same cutoff
/// when it puts an in-progress episode first.
pub const MIN_RESUME: f64 = 30.0;
/// The lowest and highest accepted watched threshold, in percent (OD-5).
const THRESHOLD_MIN: u8 = 50;
const THRESHOLD_MAX: u8 = 100;
/// The library row kind of a regular, numbered episode.
const REGULAR_EPISODE: &str = "episode";
/// The first continuity key reserved for files outside the episode run.
/// Real episode numbers are far smaller, so existing rows keep their meaning.
const NON_EPISODE_KEY_BASE: u32 = 1_000_000;

/// The kind of file a play request opens (#194).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Library tab plays exactly these file kinds"
)]
pub enum PlaybackFileKind {
    /// A numbered episode of the main run, or a movie listed with it.
    Regular,
    /// A special or OVA.
    Special,
    /// A creditless opening.
    Ncop,
    /// A creditless ending.
    Nced,
}

impl PlaybackFileKind {
    /// The continuity row of the `number`th file of this kind.
    #[must_use]
    pub fn continuity_key(self, number: u32) -> u32 {
        let rank: u32 = match self {
            Self::Regular => 0,
            Self::Special => 1,
            Self::Ncop => 2,
            Self::Nced => 3,
        };
        rank.saturating_mul(NON_EPISODE_KEY_BASE)
            .saturating_add(number)
    }

    /// Whether this file moves list progress and offers the next episode.
    #[must_use]
    pub fn is_regular(self) -> bool {
        matches!(self, Self::Regular)
    }
}

/// One request to play an episode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlayRequest {
    /// The media the episode belongs to.
    pub media: MediaId,
    /// The episode's number in the main run; zero for a special or NC file.
    pub episode: u32,
    /// What the player should open.
    pub source: StreamSource,
    /// Labels for the HUD, supplied by the screen that requested play.
    pub details: Option<PlaybackDetails>,
    /// The continuity row this file resumes from and saves to.
    pub continuity_key: u32,
    /// The kind of file; only episodes sync progress and offer auto-next.
    pub kind: PlaybackFileKind,
    /// The auto-select search that found this episode's torrent stream; a
    /// preload searches one episode on (#208 Q4).
    pub auto_select: Option<AutoSelectRequest>,
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
            continuity_key: episode,
            kind: PlaybackFileKind::Regular,
            auto_select: None,
        }
    }

    /// Plays a special. Its number names the file, never a list episode.
    #[must_use]
    pub fn special(media: MediaId, number: u32, source: StreamSource) -> Self {
        Self::non_episode(media, PlaybackFileKind::Special, number, source)
    }

    /// Plays a creditless opening. Its number names the file, never an episode.
    #[must_use]
    pub fn ncop(media: MediaId, number: u32, source: StreamSource) -> Self {
        Self::non_episode(media, PlaybackFileKind::Ncop, number, source)
    }

    /// Plays a creditless ending. Its number names the file, never an episode.
    #[must_use]
    pub fn nced(media: MediaId, number: u32, source: StreamSource) -> Self {
        Self::non_episode(media, PlaybackFileKind::Nced, number, source)
    }

    fn non_episode(
        media: MediaId,
        kind: PlaybackFileKind,
        number: u32,
        source: StreamSource,
    ) -> Self {
        Self {
            media,
            episode: 0,
            source,
            details: None,
            continuity_key: kind.continuity_key(number),
            kind,
            auto_select: None,
        }
    }

    /// Attaches the labels the HUD shows while this episode plays.
    #[must_use]
    pub fn with_details(mut self, details: PlaybackDetails) -> Self {
        self.details = Some(details);
        self
    }

    /// Attaches the auto-select search that found this episode's stream.
    #[must_use]
    pub fn with_auto_select(mut self, request: AutoSelectRequest) -> Self {
        self.auto_select = Some(request);
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

    /// Shows a short message in the player's on-screen display.
    fn show_text<'a>(&'a self, text: &'a str) -> SessionFuture<'a, Result<(), PlayerError>>;

    /// Asks the player to quit.
    fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>>;

    /// Replaces the playing file with `path` in the same window. Returns
    /// `Ok(false)` when this player needs a fresh launch instead.
    fn load_file<'a>(&'a self, path: &'a Path) -> SessionFuture<'a, Result<bool, PlayerError>>;
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

    fn show_text<'a>(&'a self, text: &'a str) -> SessionFuture<'a, Result<(), PlayerError>> {
        Box::pin(async move { MpvSession::show_text(self, text).await })
    }

    fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
        Box::pin(async move { MpvSession::quit(self).await })
    }

    fn load_file<'a>(&'a self, path: &'a Path) -> SessionFuture<'a, Result<bool, PlayerError>> {
        Box::pin(async move {
            MpvSession::load_file(self, &path.to_string_lossy())
                .await
                .map(|()| true)
        })
    }
}

/// A VLC or MPC-HC session: the next episode gets a fresh launch (#131).
#[derive(Debug)]
struct RelaunchSession(MpvSession);

impl PlaybackSession for RelaunchSession {
    fn next_event(&mut self) -> SessionFuture<'_, Option<PlayerEvent>> {
        PlaybackSession::next_event(&mut self.0)
    }

    fn seek(&self, seconds: f64) -> SessionFuture<'_, Result<(), PlayerError>> {
        PlaybackSession::seek(&self.0, seconds)
    }

    fn set_playback_state(
        &self,
        state: PlaybackControl,
    ) -> SessionFuture<'_, Result<(), PlayerError>> {
        PlaybackSession::set_playback_state(&self.0, state)
    }

    fn show_text<'a>(&'a self, text: &'a str) -> SessionFuture<'a, Result<(), PlayerError>> {
        PlaybackSession::show_text(&self.0, text)
    }

    fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
        PlaybackSession::quit(&self.0)
    }

    fn load_file<'a>(&'a self, _path: &'a Path) -> SessionFuture<'a, Result<bool, PlayerError>> {
        Box::pin(async { Ok(false) })
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
    /// The relay URL the player opens for a torrent source.
    pub stream_url: Option<String>,
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
            let file = match (&launch.source, launch.stream_url) {
                (StreamSource::Local { path }, _) => path.to_string_lossy().into_owned(),
                (StreamSource::Torrent { .. }, Some(url)) => url,
                _ => {
                    return Err(PlayerError::Endpoint(
                        "this source has no URL the player can open".to_owned(),
                    ));
                }
            };
            let (kind, executable) = Self::resolve(&launch.choice).await?;
            let mut options = if kind == PlayerKind::Iina {
                LaunchOptions::iina(launch.cache_dir)
            } else {
                LaunchOptions::new(launch.cache_dir)
            };
            options.media = Some(launch.media);
            options.subtitles = launch.subtitles;
            // mpv and IINA wait out a long stall on missing pieces; VLC and
            // MPC-HC keep their defaults (#179 Q4).
            if matches!(launch.source, StreamSource::Torrent { .. }) {
                options.network_timeout = Some(stream::NETWORK_TIMEOUT);
            }
            let boxed: Box<dyn PlaybackSession> = match kind {
                PlayerKind::Mpv => Box::new(launch_mpv(&executable, &file, &options)?),
                PlayerKind::Iina => Box::new(launch_iina(&executable, &file, &options)?),
                PlayerKind::Vlc => Box::new(RelaunchSession(
                    launch_vlc(&executable, &file, &options).await?,
                )),
                PlayerKind::MpcHc => Box::new(RelaunchSession(
                    launch_mpchc(
                        &executable,
                        &launch.mpchc_host,
                        launch.mpchc_port,
                        &file,
                        &options,
                    )
                    .await?,
                )),
                _ => {
                    return Err(PlayerError::Endpoint("unsupported player kind".to_owned()));
                }
            };
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
    /// The torrent could not be added, buffered or streamed.
    #[error("could not stream the torrent: {0}")]
    Torrent(#[from] torrent::Error),
    /// The loopback relay could not start.
    #[error("could not start the stream relay: {0}")]
    Relay(#[from] relay::Error),
    /// This manager has no torrent session to stream from.
    #[error("torrent streaming is not available")]
    TorrentUnavailable,
    /// The source kind cannot be played yet.
    #[error("this source cannot be played yet")]
    UnsupportedSource,
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
    /// Skip the chapter currently playing.
    SkipChapter,
    /// Play the next episode now instead of waiting for the countdown.
    PlayNext,
    /// Cancel the next-episode countdown.
    CancelAutoNext,
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
    /// A torrent stream is preparing (#179 Q2); `cancel` drops it, and
    /// `settled` fires once it is cleaned up or running.
    Preparing {
        id: u64,
        cancel: oneshot::Sender<()>,
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
            Self::Launching { id, .. } | Self::Preparing { id, .. } | Self::Running { id, .. } => {
                *id
            }
        }
    }
}

#[derive(Debug)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent per-episode flags, each set once by a different event"
)]
struct SessionState {
    resume: Option<db::Continuity>,
    duration: Option<f64>,
    position: f64,
    paused: bool,
    watched: bool,
    /// EOF was seen, so a following player exit is not an unexpected quit.
    eof: bool,
    /// Whether the episode is over and the next one was offered.
    end: EpisodeEnd,
    /// The next episode's preload was started (#208 Q3: once per session).
    preload_started: bool,
}

/// How far the episode is from offering the next one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EpisodeEnd {
    /// Still playing.
    Playing,
    /// Over: EOF, or the last chapter, an ending, was skipped.
    Finished,
    /// The next episode was looked up once; it is not offered again.
    Offered,
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
            end: EpisodeEnd::Playing,
            preload_started: false,
        }
    }

    /// Marks the episode over, unless the next one was already offered.
    fn finish(&mut self) {
        if self.end == EpisodeEnd::Playing {
            self.end = EpisodeEnd::Finished;
        }
    }
}

/// The chapters of the running session and what was already skipped.
#[derive(Debug, Default)]
struct ChapterState {
    /// The file's own chapters, kept so a new player list never replaces them.
    container: Vec<PlaybackChapter>,
    /// The list in use: the container's, else the player's.
    chapters: Vec<PlaybackChapter>,
    current: Option<usize>,
    skipped: Option<usize>,
}

impl ChapterState {
    fn new(container: Vec<PlaybackChapter>) -> Self {
        Self {
            chapters: chapters::with_ends(container.clone(), None),
            container,
            ..Self::default()
        }
    }
}

/// The running next-episode countdown.
#[derive(Debug)]
struct Countdown {
    next: PlayRequest,
    remaining: u8,
    tick: tokio::time::Interval,
}

/// Waits for the countdown's next step, or forever when none is running.
async fn countdown_step(countdown: &mut Option<Countdown>) {
    match countdown {
        Some(countdown) => {
            countdown.tick.tick().await;
        }
        None => std::future::pending().await,
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
    skip_op_ed: bool,
    patterns: Arc<chapters::SkipPatterns>,
    /// Countdown length in steps, `None` when auto-next is off.
    auto_next: Option<u8>,
    save_interval: Duration,
    /// One countdown step: a second, shorter in tests.
    countdown_step: Duration,
    next_id: AtomicU64,
    /// Serializes play and stop so a stop during a launch is never lost.
    commands: Arc<tokio::sync::Mutex<()>>,
    current: Arc<Mutex<Option<SessionControl>>>,
    /// The torrent session and relay; `None` refuses torrent sources.
    torrent: Option<Arc<TorrentStreams>>,
    /// How often the `Preparing` phase is reported: 4 Hz.
    prepare_tick: Duration,
    /// How long without a peer before "No peers found" (#179 Q2).
    no_peers_after: Duration,
    /// The `Preload next episode` setting (#208 Q1).
    preload_enabled: AtomicBool,
    /// Auto-select and air dates; `None` preloads nothing.
    preload_deps: Option<preload::PreloadDeps>,
    /// The one preload (#208 Q7).
    preload: Mutex<Option<preload::Preload>>,
    /// Whether the current torrent stream is buffering, for the yield.
    buffering: tokio::sync::watch::Sender<bool>,
    /// How long the current stream must go without buffering before a
    /// yielded preload resumes: 10 s.
    preload_resume_after: Duration,
    /// How long a preload is kept after the player closes: 10 min.
    preload_grace: Duration,
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
            skip_op_ed: settings.skip_op_ed,
            patterns: Arc::new(chapters::SkipPatterns::new(
                &settings.opening_patterns,
                &settings.ending_patterns,
            )),
            auto_next: settings
                .auto_next
                .then(|| settings.auto_next_seconds.max(1)),
            save_interval: Duration::from_secs(5),
            countdown_step: Duration::from_secs(1),
            next_id: AtomicU64::new(1),
            commands: Arc::new(tokio::sync::Mutex::new(())),
            current: Arc::new(Mutex::new(None)),
            torrent: None,
            prepare_tick: stream::PREPARE_TICK,
            no_peers_after: stream::NO_PEERS_AFTER,
            preload_enabled: AtomicBool::new(settings.preload_next),
            preload_deps: None,
            preload: Mutex::new(None),
            buffering: tokio::sync::watch::Sender::new(false),
            preload_resume_after: preload::RESUME_AFTER,
            preload_grace: preload::GRACE,
        }
    }

    /// Streams torrent sources through `streams`.
    #[must_use]
    pub fn with_torrent(mut self, streams: Arc<TorrentStreams>) -> Self {
        self.torrent = Some(streams);
        self
    }

    /// Preloads the next episode with `selector`, `airing` and the
    /// auto-select `profiles` (#224).
    #[must_use]
    pub fn with_preload(
        mut self,
        selector: Arc<std::sync::RwLock<AutoSelector>>,
        airing: Arc<dyn EpisodeAiring>,
        profiles: settings::AutoSelectSettings,
    ) -> Self {
        self.preload_deps = Some(preload::PreloadDeps {
            selector,
            airing,
            profiles: Mutex::new(profiles),
        });
        self
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
    pub async fn play(self: &Arc<Self>, mut request: PlayRequest) -> Result<(), PlaybackError> {
        let _command = self.commands.lock().await;
        // A play of the preloaded episode consumes it; any other drops it
        // (#208 Q7). A library file still wins over the preload.
        let consumed = if matches!(request.source, StreamSource::Torrent { .. }) {
            self.take_preload(request.media, request.episode)
        } else {
            None
        };
        if consumed.is_none() {
            self.drop_preload().await;
        }
        self.stop_inner().await;
        if let Some(fetch) = consumed {
            request.source = fetch.source();
        }
        let result = self.play_stopped(request).await;
        if let Some(fetch) = consumed {
            // Playback holds the torrent now, so this never drops it.
            self.release_preload_hold(fetch).await;
        }
        result
    }

    /// The body of [`Self::play`] once the previous session is stopped.
    async fn play_stopped(self: &Arc<Self>, request: PlayRequest) -> Result<(), PlaybackError> {
        let resume = self
            .db
            .continuity()
            .get(request.media, request.continuity_key)
            .await?;
        if let StreamSource::Torrent {
            info_hash,
            file_index,
        } = request.source
        {
            return self.start_torrent(request, info_hash, file_index, resume);
        }
        let StreamSource::Local { path } = &request.source else {
            return Err(PlaybackError::UnsupportedSource);
        };
        let container_chapters = self.container_chapters(path).await;
        let id = self.next_id.fetch_add(1, Ordering::SeqCst);
        let (settle, settled) = oneshot::channel();
        *lock(&self.current) = Some(SessionControl::Launching { id, settled });
        let launch = self.player_launch(&request, sidecar_subtitles(path));
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
                .drive(
                    id,
                    session,
                    request,
                    resume,
                    container_chapters,
                    stopped,
                    controls,
                    done,
                    None,
                )
                .await;
        });
        Ok(())
    }

    /// The chapters of a local Matroska file. Reading EBML metadata is
    /// blocking I/O, so it runs off the runtime's worker threads. A failure
    /// means no container chapters; the player's chapter-list still supplies
    /// them.
    async fn container_chapters(&self, path: &Path) -> Vec<PlaybackChapter> {
        let path = path.to_path_buf();
        let patterns = Arc::clone(&self.patterns);
        tokio::task::spawn_blocking(move || {
            chapters::read_mkv(path.as_path(), &patterns).unwrap_or_default()
        })
        .await
        .unwrap_or_default()
    }

    /// What the launcher needs to open `request` with `subtitles`.
    fn player_launch(&self, request: &PlayRequest, subtitles: Vec<PathBuf>) -> PlayerLaunch {
        PlayerLaunch {
            choice: self.player.clone(),
            source: request.source.clone(),
            stream_url: None,
            subtitles,
            cache_dir: self.cache_dir.clone(),
            media: request.media,
            mpchc_host: self.mpchc_host.clone(),
            mpchc_port: self.mpchc_port,
        }
    }

    /// Stops the active session and waits for its final position to be
    /// saved. The preload is dropped too (#208 Q7).
    pub async fn stop(&self) {
        let _command = self.commands.lock().await;
        self.drop_preload().await;
        self.stop_inner().await;
    }

    /// The body of [`Self::stop`], called with the command lock held.
    async fn stop_inner(&self) {
        let control = lock(&self.current).take();
        match control {
            Some(SessionControl::Launching { settled, .. }) => {
                let _ = settled.await;
            }
            Some(SessionControl::Preparing {
                cancel, settled, ..
            }) => {
                let _ = cancel.send(());
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

    /// Skips the chapter currently playing, when the player is in one.
    pub fn skip_chapter(&self) -> bool {
        self.send_control(Control::SkipChapter)
    }

    /// Plays the next episode now, during its countdown.
    pub fn play_next_now(&self) -> bool {
        self.send_control(Control::PlayNext)
    }

    /// Cancels the next-episode countdown, leaving the player as it is.
    pub fn cancel_auto_next(&self) -> bool {
        self.send_control(Control::CancelAutoNext)
    }

    /// Retries a failed progress sync for `episode`.
    pub async fn retry_sync(&self, media: MediaId, episode: u32) {
        self.sync_progress(media, episode).await;
    }

    #[expect(
        clippy::too_many_arguments,
        reason = "the session loop owns its player, stop signal, HUD commands, completion signal and torrent stream"
    )]
    #[expect(
        clippy::too_many_lines,
        reason = "one select loop; each arm is already a call"
    )]
    async fn drive(
        self: &Arc<Self>,
        id: u64,
        mut session: Box<dyn PlaybackSession>,
        mut request: PlayRequest,
        resume: Option<db::Continuity>,
        container_chapters: Vec<PlaybackChapter>,
        mut stopped: oneshot::Receiver<()>,
        mut controls: mpsc::UnboundedReceiver<Control>,
        done: oneshot::Sender<()>,
        mut stream: Option<LiveStream>,
    ) {
        let mut state = SessionState::new(resume);
        let mut chapter_state = ChapterState::new(container_chapters);
        self.publish_chapters(&chapter_state);
        let mut tick = tokio::time::interval(self.save_interval);
        tick.tick().await; // The first tick is immediate; nothing to save yet.
        let mut unexpected = false;
        // The player went away on its own, rather than a Stop or a new play.
        let mut closed = false;
        let mut countdown: Option<Countdown> = None;
        loop {
            enum Step {
                Event(Option<PlayerEvent>),
                Stop,
                Control(Control),
                Tick,
                Countdown,
                Buffering(bool),
            }
            let step = tokio::select! {
                event = session.next_event() => Step::Event(event),
                result = &mut stopped => {
                    let _ = result;
                    Step::Stop
                }
                // A closed channel disables this branch; without the pattern
                // it would resolve to `None` forever and spin on every pass.
                Some(control) = controls.recv() => Step::Control(control),
                _ = tick.tick() => Step::Tick,
                () = countdown_step(&mut countdown) => Step::Countdown,
                stalled = stream::buffering_change(stream.as_mut()) => Step::Buffering(stalled),
            };
            let play_next = match step {
                Step::Event(None) => {
                    unexpected = !state.eof;
                    closed = true;
                    break;
                }
                Step::Stop => break,
                Step::Control(control) => {
                    if matches!(control, Control::CancelAutoNext) && countdown.is_some() {
                        // Kept for the grace period (#208 Q7).
                        self.start_grace();
                    }
                    self.control(
                        session.as_ref(),
                        &mut state,
                        &mut chapter_state,
                        &request,
                        &mut countdown,
                        control,
                    )
                    .await
                }
                Step::Event(Some(event)) => {
                    if matches!(event, PlayerEvent::Exited) {
                        unexpected = !state.eof;
                        closed = true;
                    }
                    if !self
                        .handle(
                            session.as_ref(),
                            &mut state,
                            &mut chapter_state,
                            &request,
                            event,
                        )
                        .await
                    {
                        break;
                    }
                    self.preload_when_halfway(&mut state, &request).await;
                    None
                }
                Step::Tick => {
                    self.on_tick(&state, &request, stream.as_mut()).await;
                    None
                }
                Step::Buffering(stalled) => {
                    let _ = self.events.send(AppEvent::PlaybackBuffering(stalled));
                    self.set_stream_buffering(stalled);
                    None
                }
                Step::Countdown => self.count_down(&mut countdown),
            };
            if state.end == EpisodeEnd::Finished {
                state.end = EpisodeEnd::Offered;
                countdown = self.offer_next(&request).await;
            }
            if let Some(next) = play_next {
                match self
                    .switch_to(
                        &mut session,
                        &mut state,
                        &mut chapter_state,
                        &mut request,
                        &mut stream,
                        &mut stopped,
                        next,
                    )
                    .await
                {
                    Switch::Done => {}
                    Switch::Failed => {
                        unexpected = true;
                        break;
                    }
                    Switch::Stopped => break,
                }
            }
        }
        if self
            .end_session(id, session.as_ref(), &state, &request, stream)
            .await
        {
            let _ = self.events.send(AppEvent::PlaybackStopped { unexpected });
        }
        if closed {
            self.start_grace();
        }
        let _ = done.send(());
    }

    /// Starts the next episode's preload once the position reaches half the
    /// duration; a seek past it counts (#208 Q3).
    async fn preload_when_halfway(
        self: &Arc<Self>,
        state: &mut SessionState,
        request: &PlayRequest,
    ) {
        if state.preload_started {
            return;
        }
        let Some(duration) = state.duration else {
            return;
        };
        if state.position >= preload::PRELOAD_AT * duration {
            state.preload_started = true;
            self.start_preload(request).await;
        }
    }

    /// Saves the position, and for a torrent stream moves the relay to the
    /// finished file on disk.
    async fn on_tick(
        &self,
        state: &SessionState,
        request: &PlayRequest,
        stream: Option<&mut LiveStream>,
    ) {
        if !state.watched {
            self.save_position(state, request).await;
        }
        if let Some(stream) = stream {
            stream.switch_to_disk_when_complete().await;
        }
    }

    /// Saves or clears the position, quits the player, ends a torrent
    /// stream and releases the slot. Returns whether this was still the
    /// current session, so the HUD should hear that it ended.
    async fn end_session(
        &self,
        id: u64,
        session: &dyn PlaybackSession,
        state: &SessionState,
        request: &PlayRequest,
        stream: Option<LiveStream>,
    ) -> bool {
        if state.watched {
            self.clear_position(request).await;
        } else {
            self.save_position(state, request).await;
        }
        if let Err(error) = session.quit().await {
            debug!(%error, "player quit after the session ended");
        }
        if let Some(stream) = stream {
            stream.end().await;
            let _ = self.events.send(AppEvent::PlaybackBuffering(false));
            self.set_stream_buffering(false);
        }
        let _ = self.events.send(AppEvent::PlaybackNoRelease(None));
        let is_current = lock(&self.current)
            .as_ref()
            .is_some_and(|control| control.id() == id);
        if is_current {
            lock(&self.current).take();
        }
        is_current
    }

    /// Runs one HUD command. Returns the next episode when Play now ended
    /// its countdown.
    async fn control(
        &self,
        session: &dyn PlaybackSession,
        state: &mut SessionState,
        chapter_state: &mut ChapterState,
        request: &PlayRequest,
        countdown: &mut Option<Countdown>,
        control: Control,
    ) -> Option<PlayRequest> {
        let result = match control {
            Control::SetPlaybackState(mode) => session.set_playback_state(mode).await,
            Control::Seek(seconds) => {
                if seconds.is_finite() && seconds >= 0.0 {
                    state.position = seconds;
                    self.publish(state, request);
                }
                session.seek(seconds).await
            }
            Control::SkipChapter => {
                self.skip_current(session, state, chapter_state).await;
                Ok(())
            }
            Control::PlayNext => return countdown.take().map(|countdown| countdown.next),
            Control::CancelAutoNext => {
                if countdown.take().is_some() {
                    self.publish_auto_next(None);
                }
                Ok(())
            }
        };
        if let Err(error) = result {
            debug!(%error, "player rejected a HUD command");
        }
        None
    }

    /// One countdown step. Returns the next episode when it reaches zero.
    fn count_down(&self, countdown: &mut Option<Countdown>) -> Option<PlayRequest> {
        let running = countdown.as_mut()?;
        running.remaining = running.remaining.saturating_sub(1);
        if running.remaining == 0 {
            return countdown.take().map(|countdown| countdown.next);
        }
        self.publish_auto_next(Some(running));
        None
    }

    /// Returns `false` when the session is over.
    async fn handle(
        &self,
        session: &dyn PlaybackSession,
        state: &mut SessionState,
        chapter_state: &mut ChapterState,
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
                    chapter_state.current = chapter_current(&chapter_state.chapters, seconds);
                    if self.skip_op_ed {
                        self.skip_current_if_entered(session, state, chapter_state)
                            .await;
                    }
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
                if !stream::eof_is_the_end(&request.source, state) {
                    // A torrent read failed mid-file: keep the position.
                    warn!(
                        position = state.position,
                        duration = state.duration,
                        "the torrent stream ended before the end of the file"
                    );
                    return true;
                }
                state.eof = true;
                state.finish();
                self.mark_watched(state, request).await;
            }
            PlayerEvent::Exited => return false,
            PlayerEvent::Chapters { list, .. } => {
                self.update_chapters(chapter_state, state.position, &list);
                self.publish_chapters(chapter_state);
                if self.skip_op_ed {
                    self.skip_current_if_entered(session, state, chapter_state)
                        .await;
                }
            }
            _ => {}
        }
        true
    }

    /// Keeps the container's chapters when it had any; otherwise falls back
    /// to the player's `chapter-list`. The current index is resolved against
    /// the chosen list, never against the player's list: mpv's index may
    /// point at a different chapter when the two lists differ.
    fn update_chapters(
        &self,
        chapter_state: &mut ChapterState,
        position: f64,
        player_list: &[player::Chapter],
    ) {
        let from_player = chapters::from_player(player_list, &self.patterns);
        let chosen = chapters::with_ends(
            chapters::prefer_mkv(chapter_state.container.clone(), from_player),
            None,
        );
        // A different list makes the skipped index point at another chapter.
        if chosen != chapter_state.chapters {
            chapter_state.skipped = None;
        }
        chapter_state.chapters = chosen;
        chapter_state.current = chapter_current(&chapter_state.chapters, position);
    }

    fn publish_chapters(&self, chapter_state: &ChapterState) {
        let _ = self.events.send(AppEvent::PlaybackChapters {
            chapters: chapter_state.chapters.clone(),
            current: chapter_state.current,
        });
    }

    /// Seeks past the current chapter once, with a short player message.
    async fn skip_current(
        &self,
        session: &dyn PlaybackSession,
        state: &mut SessionState,
        chapter_state: &mut ChapterState,
    ) {
        let Some(index) = chapter_state.current else {
            return;
        };
        let Some(chapter) = chapter_state.chapters.get(index) else {
            return;
        };
        let Some(kind) = chapter.kind else {
            return;
        };
        let to = chapter
            .end
            .or_else(|| chapter_state.chapters.get(index + 1).map(|next| next.start))
            .or(state.duration.map(Duration::from_secs_f64));
        let Some(to) = to else {
            return;
        };
        // Mark the attempt even when the seek fails: otherwise the 4 Hz
        // position events would retry it continuously.
        chapter_state.skipped = Some(index);
        let last = chapter_state.chapters.get(index + 1).is_none();
        match session.seek(to.as_secs_f64()).await {
            Ok(()) => {
                state.position = to.as_secs_f64();
                // Nothing follows a skipped ending: the episode is over.
                if last && kind == domain::ChapterKind::Ending {
                    state.finish();
                }
                let message = match kind {
                    domain::ChapterKind::Opening => "Skipped opening",
                    domain::ChapterKind::Ending => "Skipped ending",
                    _ => "Skipped chapter",
                };
                if let Err(error) = session.show_text(message).await {
                    debug!(%error, "could not show the chapter skip message");
                }
            }
            Err(error) => debug!(%error, "could not skip the chapter"),
        }
    }

    /// Looks up the next regular episode and, when auto-next is on and it
    /// exists, starts its countdown: the library first, then for a torrent
    /// stream the same batch, then the preload (#179 Q5).
    async fn offer_next(&self, request: &PlayRequest) -> Option<Countdown> {
        if !request.kind.is_regular() {
            return None;
        }
        let seconds = self.auto_next?;
        let next = match self.next_episode(request).await {
            Some(next) => next,
            None => self.next_stream(request).await?,
        };
        let step = self.countdown_step;
        let mut countdown = Countdown {
            next,
            remaining: seconds,
            tick: tokio::time::interval_at(tokio::time::Instant::now() + step, step),
        };
        countdown
            .tick
            .set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        self.publish_auto_next(Some(&countdown));
        Some(countdown)
    }

    /// The library file of the episode after `request`, as a play request
    /// that keeps the media's HUD labels.
    async fn next_episode(&self, request: &PlayRequest) -> Option<PlayRequest> {
        let episode = request.episode.checked_add(1)?;
        let files = match library::files_for(&self.db, request.media).await {
            Ok(files) => files,
            Err(error) => {
                warn!(%error, "could not look up the next episode");
                return None;
            }
        };
        let file = files.into_iter().find(|file| {
            file.kind == REGULAR_EPISODE
                && file.episode == Some(episode)
                && !file.ignored
                && !file.shelved
        })?;
        Some(next_request(
            request,
            episode,
            StreamSource::local(file.path),
        ))
    }

    /// The torrent stream of the episode after a torrent stream: the same
    /// batch torrent, else the preload. A failed preload is shown now, at
    /// episode end, and nothing is offered (#208 Q4).
    async fn next_stream(&self, request: &PlayRequest) -> Option<PlayRequest> {
        let StreamSource::Torrent {
            info_hash,
            file_index,
        } = request.source
        else {
            return None;
        };
        let episode = request.episode.checked_add(1)?;
        let outcome = self.preload_outcome(request.media, episode);
        if let Some(preload::Outcome::Ready(fetch)) = outcome {
            return Some(next_request(request, episode, fetch.source()));
        }
        let session = self.torrent.as_ref()?.session().await.ok()?;
        if let Some(next) = preload::batch_file(&session, info_hash, file_index, request).await {
            return Some(next_request(
                request,
                episode,
                StreamSource::torrent(info_hash, next),
            ));
        }
        if let Some(preload::Outcome::Failed(reason)) = outcome {
            let _ = self
                .events
                .send(AppEvent::PlaybackNoRelease(Some(domain::NoRelease::new(
                    request.media,
                    episode,
                    reason,
                ))));
        }
        None
    }

    fn publish_auto_next(&self, countdown: Option<&Countdown>) {
        let next = countdown.map(|countdown| {
            AutoNext::new(
                countdown.next.media,
                countdown.next.episode,
                Duration::from_secs(u64::from(countdown.remaining)),
            )
        });
        let _ = self.events.send(AppEvent::PlaybackAutoNext(next));
    }

    /// Moves the session to `next`: the player replaces its file when it
    /// can, otherwise it is relaunched. A torrent stream first goes through
    /// `Preparing`, which a stop interrupts.
    #[expect(
        clippy::too_many_arguments,
        reason = "the switch replaces the session loop's player, state, chapters, request and stream"
    )]
    async fn switch_to(
        &self,
        session: &mut Box<dyn PlaybackSession>,
        state: &mut SessionState,
        chapter_state: &mut ChapterState,
        request: &mut PlayRequest,
        stream: &mut Option<LiveStream>,
        stopped: &mut oneshot::Receiver<()>,
        next: PlayRequest,
    ) -> Switch {
        self.publish_auto_next(None);
        self.mark_watched(state, request).await;
        let resume = match self
            .db
            .continuity()
            .get(next.media, next.continuity_key)
            .await
        {
            Ok(resume) => resume,
            Err(error) => {
                warn!(%error, "could not read the next episode's position");
                None
            }
        };
        let container_chapters = match &next.source {
            StreamSource::Local { path } => {
                // A library file needs no relay: the old stream ends.
                if let Some(old) = stream.take() {
                    old.end().await;
                    self.set_stream_buffering(false);
                }
                let container_chapters = self.container_chapters(path).await;
                if !self
                    .open_next(session, &next, path, sidecar_subtitles(path), None)
                    .await
                {
                    return Switch::Failed;
                }
                container_chapters
            }
            StreamSource::Torrent {
                info_hash,
                file_index,
            } => {
                match self
                    .switch_stream(session, stream, stopped, &next, *info_hash, *file_index)
                    .await
                {
                    Switch::Done => Vec::new(),
                    other => return other,
                }
            }
            _ => return Switch::Failed,
        };
        *state = SessionState::new(resume);
        *chapter_state = ChapterState::new(container_chapters);
        *request = next;
        self.publish_chapters(chapter_state);
        self.publish(state, request);
        Switch::Done
    }

    /// Opens `next` at `file` in the running player, or relaunches it on
    /// `next` (with `stream_url` for a torrent stream). Returns `false` when
    /// no player is left to drive.
    async fn open_next(
        &self,
        session: &mut Box<dyn PlaybackSession>,
        next: &PlayRequest,
        file: &Path,
        subtitles: Vec<PathBuf>,
        stream_url: Option<String>,
    ) -> bool {
        let replaced = match session.load_file(file).await {
            Ok(replaced) => replaced,
            Err(error) => {
                debug!(%error, "the player could not replace its file; relaunching");
                false
            }
        };
        // mpv's keep-open pauses at EOF, and the pause carries over to the
        // replaced file.
        if replaced && let Err(error) = session.set_playback_state(PlaybackControl::Resume).await {
            debug!(%error, "could not resume the next episode");
        }
        if replaced {
            return true;
        }
        if let Err(error) = session.quit().await {
            debug!(%error, "player quit before the next episode");
        }
        let mut launch = self.player_launch(next, subtitles);
        launch.stream_url = stream_url;
        match self.launcher.launch(launch).await {
            Ok(fresh) => {
                *session = fresh;
                true
            }
            Err(error) => {
                warn!(%error, "could not launch the next episode");
                false
            }
        }
    }

    /// Auto-skip only when the position is inside a matching chapter.
    async fn skip_current_if_entered(
        &self,
        session: &dyn PlaybackSession,
        state: &mut SessionState,
        chapter_state: &mut ChapterState,
    ) {
        let Some(index) = chapter_state.current else {
            return;
        };
        if chapter_state.skipped == Some(index) {
            return;
        }
        let Some(chapter) = chapter_state.chapters.get(index) else {
            return;
        };
        if chapter.kind.is_none() || state.position < chapter.start.as_secs_f64() {
            return;
        }
        if let Some(end) = chapter.end
            && state.position >= end.as_secs_f64()
        {
            return;
        }
        self.skip_current(session, state, chapter_state).await;
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
        if self.auto_sync && request.kind.is_regular() {
            self.sync_progress(request.media, request.episode).await;
        }
    }

    async fn save_position(&self, state: &SessionState, request: &PlayRequest) {
        let result = self
            .db
            .continuity()
            .save(
                request.media,
                request.continuity_key,
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
            .clear(request.media, request.continuity_key)
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

/// How a switch to the next episode ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Switch {
    /// The next episode plays.
    Done,
    /// No player is left to drive.
    Failed,
    /// A stop arrived while the next stream prepared.
    Stopped,
}

/// The play request for `episode`, the one after `request`, from `source`.
/// It keeps the media's HUD labels and searches one episode on.
fn next_request(request: &PlayRequest, episode: u32, source: StreamSource) -> PlayRequest {
    let details = request.details.as_ref().map(|details| {
        PlaybackDetails::new(details.media_title.clone(), None, details.poster.clone())
    });
    PlayRequest {
        media: request.media,
        episode,
        source,
        details,
        continuity_key: episode,
        kind: PlaybackFileKind::Regular,
        auto_select: preload::next_search(request),
    }
}

/// The chapter containing `position`, in seconds, in the chosen chapter list.
fn chapter_current(chapters: &[PlaybackChapter], position: f64) -> Option<usize> {
    if !position.is_finite() || position < 0.0 {
        return None;
    }
    let at = Duration::from_secs_f64(position);
    chapters
        .iter()
        .rposition(|chapter| chapter.start <= at && chapter.end.is_none_or(|end| at < end))
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
pub(crate) mod fakes;

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use domain::{ChapterKind, ExternalId, ListStatus, MediaKind};
    use futures::{SinkExt, channel::mpsc};
    use metadata::{MediaDetail, MediaSummary, MediaTitle};
    use settings::PlaybackSettings;

    use super::{fakes::FakeLauncher, *};

    fn entry(media: MediaId, progress: u32, status: ListStatus) -> ListEntry {
        new_entry(media, progress, status)
    }

    /// The Entry detail a test's catalog cache serves, with an episode count.
    fn detail(anilist_id: u32) -> MediaDetail {
        MediaDetail {
            summary: MediaSummary {
                anilist_id,
                mal_id: None,
                kind: Some(MediaKind::Anime),
                title: MediaTitle::default(),
                poster: None,
                banner: None,
                format: None,
                status: None,
                season: None,
                season_year: None,
                episodes: Some(12),
                chapters: None,
                mean_score: None,
                genres: Vec::new(),
                is_adult: false,
            },
            description: None,
            start_date: domain::FuzzyDate::UNKNOWN,
            end_date: domain::FuzzyDate::UNKNOWN,
            duration: None,
            studios: Vec::new(),
            popularity: None,
            characters: Vec::new(),
            relations: Vec::new(),
            recommendations: Vec::new(),
        }
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

    struct Fixture {
        _dir: tempfile::TempDir,
        db: Db,
        catalog: CatalogCache,
        manager: Arc<PlaybackManager>,
        sync: Arc<FakeSync>,
        launcher: FakeLauncher,
        events: flume::Receiver<AppEvent>,
        /// Kept alive so the drive loop's control branch stays open.
        control_senders: Arc<Mutex<Vec<tokio::sync::mpsc::UnboundedSender<Control>>>>,
    }

    impl Fixture {
        async fn new(settings: PlaybackSettings) -> Self {
            Self::with_interval(settings, Duration::from_secs(5)).await
        }

        /// A fixture whose countdown steps every 20 ms instead of every second.
        async fn with_fast_countdown(settings: PlaybackSettings) -> Self {
            let mut fixture = Self::new(settings).await;
            Arc::get_mut(&mut fixture.manager)
                .expect("no session holds the manager yet")
                .countdown_step = Duration::from_millis(20);
            fixture
        }

        /// Puts one regular episode file for `media` in the library.
        async fn add_episode(&self, media: MediaId, episode: u32, path: &str) {
            let mut files = self.db.local_file().all().await.unwrap();
            files.push(db::LocalFile {
                path: PathBuf::from(path),
                size: 0,
                modified: 0,
                title: None,
                group: None,
                season: None,
                episode: Some(episode),
                kind: "episode".to_owned(),
                media_id: Some(media),
                confidence: Some(1.0),
                locked: false,
                ignored: false,
                filler: false,
                shelved: false,
            });
            self.db.local_file().sync(&files).await.unwrap();
        }

        /// The countdowns published so far, `None` for a cancelled one.
        fn countdowns(&self) -> Vec<Option<AutoNext>> {
            self.events
                .try_iter()
                .filter_map(|event| match event {
                    AppEvent::PlaybackAutoNext(next) => Some(next),
                    _ => None,
                })
                .collect()
        }

        /// The HUD-command sender of the session started last.
        fn controls(&self) -> tokio::sync::mpsc::UnboundedSender<Control> {
            lock(&self.control_senders)
                .last()
                .cloned()
                .expect("a session was driven")
        }

        async fn with_interval(settings: PlaybackSettings, interval: Duration) -> Self {
            let dir = tempfile::tempdir().unwrap();
            let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
            let cache = dir.path().join("cache");
            fs::create_dir_all(&cache).unwrap();
            // A cache miss fails at once rather than reaching AniList. Tests
            // seed the entry count through `media()` below.
            let client = net::Client::builder().max_retries(0).build().unwrap();
            let anilist = metadata::AniList::with_endpoint(client, "http://127.0.0.1:1/");
            let poster = net::PosterCache::new(&cache).unwrap();
            let catalog = CatalogCache::new(anilist, poster);
            let (events, receiver) = flume::unbounded();
            let sync = Arc::new(FakeSync::default());
            let launcher = FakeLauncher::default();
            let progress: Arc<dyn ProgressSync> = Arc::<FakeSync>::clone(&sync);
            let mut manager = PlaybackManager::with_launcher(
                db.clone(),
                catalog.clone(),
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
                catalog,
                manager,
                sync,
                launcher,
                events: receiver,
                control_senders: Arc::new(Mutex::new(Vec::new())),
            }
        }

        async fn media(&self, id: u32) -> MediaId {
            // Opening the Entry page would cache the count watched sync reads.
            let detail = detail(id);
            self.catalog
                .section(
                    format!("media:{id}"),
                    Duration::from_hours(24),
                    async move { Ok(detail) },
                )
                .await
                .unwrap();
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
            self.drive_with_chapters(resume, request, Vec::new())
        }

        fn drive_with_chapters(
            &self,
            resume: Option<db::Continuity>,
            request: &PlayRequest,
            chapters: Vec<PlaybackChapter>,
        ) -> (
            mpsc::UnboundedSender<PlayerEvent>,
            oneshot::Sender<()>,
            oneshot::Receiver<()>,
        ) {
            let (session, events) = self.launcher.session();
            let (stop, stopped) = oneshot::channel();
            let (done, finished) = oneshot::channel();
            let (control, controls) = tokio::sync::mpsc::unbounded_channel::<Control>();
            lock(&self.control_senders).push(control);
            let manager = Arc::clone(&self.manager);
            let request = request.clone();
            tokio::spawn(async move {
                manager
                    .drive(
                        1, session, request, resume, chapters, stopped, controls, done, None,
                    )
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
    async fn two_specials_resume_at_their_own_positions() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let first = PlayRequest::special(media, 1, StreamSource::local("/media/sp1.mkv"));
        let second = PlayRequest::special(media, 2, StreamSource::local("/media/sp2.mkv"));

        fixture.manager.play(first.clone()).await.unwrap();
        let mut events = fixture.launcher.sender(0);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 60.0 })
            .await
            .unwrap();
        wait_until("the session to see the position", || {
            fixture
                .states()
                .iter()
                .any(|state| state.position == Duration::from_secs(60))
        })
        .await;
        fixture.manager.stop().await;

        assert_eq!(
            fixture
                .db
                .continuity()
                .get(media, first.continuity_key)
                .await
                .unwrap()
                .map(|saved| saved.position),
            Some(60.0),
            "the first special saved its own row"
        );
        assert_eq!(
            fixture
                .db
                .continuity()
                .get(media, second.continuity_key)
                .await
                .unwrap(),
            None,
            "the second special has no row yet"
        );

        fixture.manager.play(second).await.unwrap();
        let mut events = fixture.launcher.sender(1);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        tokio::task::yield_now().await;
        assert!(
            fixture.launcher.seeks().is_empty(),
            "the other special's position was not reused"
        );
        fixture.manager.stop().await;

        fixture.manager.play(first).await.unwrap();
        let mut events = fixture.launcher.sender(2);
        events
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        wait_until("the first special resumes", || {
            !fixture.launcher.seeks().is_empty()
        })
        .await;
        assert_eq!(fixture.launcher.seeks(), [60.0], "its own position resumes");
        fixture.manager.stop().await;
    }

    #[tokio::test]
    async fn a_special_past_the_threshold_never_syncs_or_creates_a_list_entry() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let request = PlayRequest::special(media, 1, StreamSource::local("/media/sp1.mkv"));
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
        events.send(PlayerEvent::Eof).await.unwrap();
        finish(events, stop, done).await;

        assert!(fixture.sync.updates().is_empty(), "no progress write");
        assert!(fixture.sync.saves().is_empty(), "no entry was saved");
        assert!(
            lock(&fixture.sync.entry).is_none(),
            "no list entry was created at zero"
        );
        assert_eq!(
            fixture
                .db
                .continuity()
                .get(media, request.continuity_key)
                .await
                .unwrap(),
            None,
            "a watched special leaves no resume row"
        );
    }

    #[tokio::test]
    async fn a_regular_episode_still_syncs_and_offers_the_next_one() {
        let fixture = Fixture::with_fast_countdown(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        fixture.add_episode(media, 1, "/media/show/01.mkv").await;
        fixture.add_episode(media, 2, "/media/show/02.mkv").await;
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 1));
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
        events.send(PlayerEvent::Eof).await.unwrap();
        wait_until("the next episode to load", || {
            !fixture.launcher.loads().is_empty()
        })
        .await;

        assert_eq!(fixture.sync.updates(), [1], "the episode moved progress");
        assert!(
            fixture
                .countdowns()
                .iter()
                .flatten()
                .all(|next| next.media == media && next.episode == 2),
            "the countdown follows the next regular episode"
        );
        finish(events, stop, done).await;
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
    async fn a_closed_control_channel_does_not_spin_and_save() {
        let fixture =
            Fixture::with_interval(PlaybackSettings::default(), Duration::from_secs(60)).await;
        let media = fixture.media(1).await;
        let request = request(media, 1);
        let (session, mut events) = fixture.launcher.session();
        let (stop, stopped) = oneshot::channel();
        let (done, finished) = oneshot::channel();
        let (control, controls) = tokio::sync::mpsc::unbounded_channel::<Control>();
        drop(control);
        let manager = Arc::clone(&fixture.manager);
        tokio::spawn(async move {
            manager
                .drive(
                    1,
                    session,
                    request,
                    None,
                    Vec::new(),
                    stopped,
                    controls,
                    done,
                    None,
                )
                .await;
        });

        for _ in 0..100 {
            tokio::task::yield_now().await;
        }
        assert_eq!(
            fixture.db.continuity().get(media, 1).await.unwrap(),
            None,
            "a closed command channel must not spin on periodic saves"
        );

        events.send(PlayerEvent::Exited).await.unwrap();
        let _ = tokio::time::timeout(Duration::from_secs(1), finished).await;
        drop(stop);
    }

    fn opening_chapter() -> PlaybackChapter {
        PlaybackChapter::new(
            Some("Opening".to_owned()),
            Duration::from_secs(10),
            Some(Duration::from_secs(90)),
            Some(ChapterKind::Opening),
        )
    }

    #[tokio::test]
    async fn entering_an_opening_chapter_skips_it_once_with_an_osd_message() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (mut events, stop, done) =
            fixture.drive_with_chapters(None, &request(media, 1), vec![opening_chapter()]);

        events
            .send(PlayerEvent::Chapters {
                current: Some(0),
                list: vec![player::Chapter {
                    title: Some("Opening".to_owned()),
                    start: 10.0,
                }],
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 12.0 })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 13.0 })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert_eq!(fixture.launcher.seeks(), [90.0], "skipped only once");
        assert_eq!(
            *lock(&fixture.launcher.messages),
            ["Skipped opening".to_owned()],
            "the player shows the OSD message"
        );
        assert!(
            fixture
                .events
                .try_iter()
                .any(|event| matches!(event, AppEvent::PlaybackChapters { .. })),
            "the HUD receives the chapter marks"
        );
    }

    #[tokio::test]
    async fn the_current_chapter_comes_from_the_chosen_list_not_mpv() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let ending = PlaybackChapter::new(
            Some("Ending".to_owned()),
            Duration::from_secs(200),
            Some(Duration::from_secs(240)),
            Some(ChapterKind::Ending),
        );
        let (mut events, stop, done) =
            fixture.drive_with_chapters(None, &request(media, 1), vec![opening_chapter(), ending]);

        events
            .send(PlayerEvent::Position { seconds: 60.0 })
            .await
            .unwrap();
        // mpv's index and timings are for a different list: index 1 points at
        // its "Ending", while 60 s is inside the container's "Opening".
        events
            .send(PlayerEvent::Chapters {
                current: Some(1),
                list: vec![
                    player::Chapter {
                        title: Some("Opening".to_owned()),
                        start: 50.0,
                    },
                    player::Chapter {
                        title: Some("Ending".to_owned()),
                        start: 150.0,
                    },
                    player::Chapter {
                        title: Some("Credits".to_owned()),
                        start: 300.0,
                    },
                ],
            })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert_eq!(
            fixture.launcher.seeks(),
            [90.0],
            "the container chapter won"
        );
        assert_eq!(
            *lock(&fixture.launcher.messages),
            ["Skipped opening".to_owned()]
        );
    }

    #[tokio::test]
    async fn auto_skip_can_be_turned_off() {
        let settings = PlaybackSettings {
            skip_op_ed: false,
            ..PlaybackSettings::default()
        };
        let fixture = Fixture::new(settings).await;
        let media = fixture.media(1).await;
        let (mut events, stop, done) =
            fixture.drive_with_chapters(None, &request(media, 1), vec![opening_chapter()]);
        events
            .send(PlayerEvent::Chapters {
                current: Some(0),
                list: Vec::new(),
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 12.0 })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert!(fixture.launcher.seeks().is_empty(), "auto-skip is off");
    }

    #[tokio::test]
    async fn a_skip_command_seeks_past_the_current_chapter() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let (session, _) = fixture.launcher.session();
        let mut state = SessionState::new(None);
        let mut chapter_state = ChapterState {
            chapters: vec![opening_chapter()],
            current: Some(0),
            ..ChapterState::default()
        };

        fixture
            .manager
            .skip_current(session.as_ref(), &mut state, &mut chapter_state)
            .await;

        assert_eq!(fixture.launcher.seeks(), [90.0]);
        assert_eq!(
            *lock(&fixture.launcher.messages),
            ["Skipped opening".to_owned()]
        );
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
            fixture.manager.set_playback_state(PlaybackControl::Pause),
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

    /// Polls `done` until it holds, failing the test after two seconds.
    async fn wait_until(what: &str, done: impl Fn() -> bool) {
        let waited = tokio::time::timeout(Duration::from_secs(2), async {
            while !done() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
        assert!(waited.is_ok(), "timed out waiting for {what}");
    }

    /// A fixture with episodes 1 and 2 of one media in the library.
    async fn two_episode_library(settings: PlaybackSettings) -> (Fixture, MediaId) {
        // Sync would look the episode count up on AniList; keep tests offline.
        let settings = PlaybackSettings {
            auto_sync_progress: false,
            ..settings
        };
        let fixture = Fixture::with_fast_countdown(settings).await;
        let media = fixture.media(1).await;
        fixture.add_episode(media, 1, "/media/show/01.mkv").await;
        fixture.add_episode(media, 2, "/media/show/02.mkv").await;
        (fixture, media)
    }

    fn episode_request(media: MediaId, episode: u32) -> PlayRequest {
        PlayRequest::new(
            media,
            episode,
            StreamSource::local(format!("/media/show/{episode:02}.mkv")),
        )
        .with_details(PlaybackDetails::new(
            "A show",
            Some(format!("Episode {episode}")),
            None,
        ))
    }

    #[tokio::test]
    async fn eof_counts_down_and_loads_the_next_episode_in_the_same_player() {
        let (fixture, media) = two_episode_library(PlaybackSettings::default()).await;
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 1));

        events.send(PlayerEvent::Eof).await.unwrap();
        wait_until("the next file to load and play", || {
            !fixture.launcher.loads().is_empty() && !fixture.launcher.pauses().is_empty()
        })
        .await;

        assert_eq!(
            fixture.launcher.loads(),
            [PathBuf::from("/media/show/02.mkv")]
        );
        assert_eq!(fixture.launcher.launches(), 0, "the same player was reused");
        assert_eq!(
            fixture.launcher.pauses(),
            [PlaybackControl::Resume],
            "keep-open paused at EOF; the next episode plays"
        );
        let countdowns = fixture.countdowns();
        let seconds: Vec<u64> = countdowns
            .iter()
            .flatten()
            .map(|next| next.remaining.as_secs())
            .collect();
        assert_eq!(seconds, [5, 4, 3, 2, 1], "a five-step countdown");
        assert!(
            countdowns
                .iter()
                .flatten()
                .all(|next| next.media == media && next.episode == 2)
        );
        assert_eq!(
            countdowns.last(),
            Some(&None),
            "the HUD countdown is cleared"
        );

        events
            .send(PlayerEvent::Position { seconds: 1.0 })
            .await
            .unwrap();
        wait_until("a position for the next episode", || {
            fixture.states().iter().any(|state| state.episode == 2)
        })
        .await;
        finish(events, stop, done).await;
    }

    #[tokio::test]
    async fn cancelling_the_countdown_stops_the_load() {
        let (fixture, media) = two_episode_library(PlaybackSettings::default()).await;
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 1));

        events.send(PlayerEvent::Eof).await.unwrap();
        wait_until("the countdown to start", || {
            fixture
                .events
                .try_iter()
                .any(|event| matches!(event, AppEvent::PlaybackAutoNext(Some(_))))
        })
        .await;
        fixture.controls().send(Control::CancelAutoNext).unwrap();
        wait_until("the countdown to be cleared", || {
            fixture
                .events
                .try_iter()
                .any(|event| matches!(event, AppEvent::PlaybackAutoNext(None)))
        })
        .await;
        // Five 20 ms steps would have finished by now.
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(fixture.launcher.loads().is_empty(), "nothing was loaded");
        assert_eq!(fixture.launcher.launches(), 0, "nothing was launched");
        finish(events, stop, done).await;
    }

    #[tokio::test]
    async fn no_countdown_on_the_last_library_episode() {
        let (fixture, media) = two_episode_library(PlaybackSettings::default()).await;
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 2));

        events.send(PlayerEvent::Eof).await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        finish(events, stop, done).await;

        assert!(
            fixture.countdowns().is_empty(),
            "no next episode, no countdown"
        );
        assert!(fixture.launcher.loads().is_empty(), "nothing was loaded");
    }

    #[tokio::test]
    async fn auto_next_off_shows_no_countdown() {
        let settings = PlaybackSettings {
            auto_next: false,
            ..PlaybackSettings::default()
        };
        let (fixture, media) = two_episode_library(settings).await;
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 1));

        events.send(PlayerEvent::Eof).await.unwrap();
        tokio::time::sleep(Duration::from_millis(250)).await;
        finish(events, stop, done).await;

        assert!(fixture.countdowns().is_empty(), "auto-next is off");
        assert!(fixture.launcher.loads().is_empty(), "nothing was loaded");
    }

    #[tokio::test]
    async fn play_now_skips_the_rest_of_the_countdown() {
        let (mut fixture, media) = two_episode_library(PlaybackSettings::default()).await;
        Arc::get_mut(&mut fixture.manager)
            .expect("no session holds the manager yet")
            .countdown_step = Duration::from_secs(60);
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 1));

        events.send(PlayerEvent::Eof).await.unwrap();
        wait_until("the countdown to start", || {
            fixture
                .events
                .try_iter()
                .any(|event| matches!(event, AppEvent::PlaybackAutoNext(Some(_))))
        })
        .await;
        fixture.controls().send(Control::PlayNext).unwrap();
        wait_until("the next file to load", || {
            !fixture.launcher.loads().is_empty()
        })
        .await;

        assert_eq!(
            fixture.launcher.loads(),
            [PathBuf::from("/media/show/02.mkv")]
        );
        finish(events, stop, done).await;
    }

    #[tokio::test]
    async fn players_without_loadfile_relaunch_for_the_next_episode() {
        let (fixture, media) = two_episode_library(PlaybackSettings::default()).await;
        fixture.launcher.relaunch.store(true, Ordering::SeqCst);
        let (mut events, stop, done) = fixture.drive(None, &episode_request(media, 1));

        events.send(PlayerEvent::Eof).await.unwrap();
        wait_until("a fresh launch", || fixture.launcher.launches() == 1).await;

        assert!(
            fixture.launcher.quits.load(Ordering::SeqCst) >= 1,
            "the old player quit first"
        );
        assert!(
            fixture.launcher.loads().is_empty(),
            "the window was not reused"
        );
        // The first player's events went with it; end the relaunched one.
        drop(events);
        let fresh = lock(&fixture.launcher.senders).last().cloned();
        finish(fresh.expect("the relaunched player"), stop, done).await;
    }

    #[tokio::test]
    async fn skipping_a_final_ending_starts_the_countdown() {
        let (fixture, media) = two_episode_library(PlaybackSettings::default()).await;
        let ending = PlaybackChapter::new(
            Some("Ending".to_owned()),
            Duration::from_secs(200),
            Some(Duration::from_secs(240)),
            Some(ChapterKind::Ending),
        );
        let (mut events, stop, done) =
            fixture.drive_with_chapters(None, &episode_request(media, 1), vec![ending]);

        events
            .send(PlayerEvent::Position { seconds: 201.0 })
            .await
            .unwrap();
        wait_until("the next file to load", || {
            !fixture.launcher.loads().is_empty()
        })
        .await;

        assert_eq!(fixture.launcher.seeks(), [240.0], "the ending was skipped");
        finish(events, stop, done).await;
    }

    #[tokio::test]
    async fn the_current_chapter_follows_a_later_position() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (mut events, stop, done) =
            fixture.drive_with_chapters(None, &request(media, 1), vec![opening_chapter()]);

        // The list arrives at 0 s, before the opening; mpv's index is wrong.
        events
            .send(PlayerEvent::Chapters {
                current: Some(3),
                list: Vec::new(),
            })
            .await
            .unwrap();
        events
            .send(PlayerEvent::Position { seconds: 60.0 })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert_eq!(
            fixture.launcher.seeks(),
            [90.0],
            "the position, not mpv's index, found the opening"
        );
    }

    #[tokio::test]
    async fn a_failed_skip_seek_is_tried_once() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        fixture.launcher.fail_seeks.store(true, Ordering::SeqCst);
        let media = fixture.media(1).await;
        let (mut events, stop, done) =
            fixture.drive_with_chapters(None, &request(media, 1), vec![opening_chapter()]);

        for seconds in [12.0, 12.25, 12.5] {
            events
                .send(PlayerEvent::Position { seconds })
                .await
                .unwrap();
        }
        finish(events, stop, done).await;

        assert_eq!(fixture.launcher.seeks(), [90.0], "no retry at 4 Hz");
        assert!(
            lock(&fixture.launcher.messages).is_empty(),
            "no OSD on failure"
        );
    }

    #[tokio::test]
    async fn a_replaced_chapter_list_can_be_skipped_again() {
        let fixture = Fixture::new(PlaybackSettings::default()).await;
        let media = fixture.media(1).await;
        let (mut events, stop, done) = fixture.drive(None, &request(media, 1));
        let opening_at = |start: f64| PlayerEvent::Chapters {
            current: Some(0),
            list: vec![
                player::Chapter {
                    title: Some("Opening".to_owned()),
                    start,
                },
                player::Chapter {
                    title: Some("Part A".to_owned()),
                    start: start + 90.0,
                },
            ],
        };

        events.send(opening_at(0.0)).await.unwrap();
        events
            .send(PlayerEvent::Position { seconds: 1.0 })
            .await
            .unwrap();
        // A different list, as after a file change: index 0 is a new opening.
        events.send(opening_at(100.0)).await.unwrap();
        events
            .send(PlayerEvent::Position { seconds: 101.0 })
            .await
            .unwrap();
        finish(events, stop, done).await;

        assert_eq!(fixture.launcher.seeks(), [90.0, 190.0]);
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

        fn show_text<'a>(&'a self, text: &'a str) -> SessionFuture<'a, Result<(), PlayerError>> {
            Box::pin(async move { self.inner.show_text(text).await })
        }

        fn quit(&self) -> SessionFuture<'_, Result<(), PlayerError>> {
            Box::pin(async move { self.inner.quit().await })
        }

        fn load_file<'a>(&'a self, path: &'a Path) -> SessionFuture<'a, Result<bool, PlayerError>> {
            PlaybackSession::load_file(&self.inner, path)
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
