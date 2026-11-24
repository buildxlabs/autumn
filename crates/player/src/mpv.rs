//! Detached, supervised mpv launch and the public [`Session`] handle.

use std::{path::PathBuf, process::Stdio, sync::OnceLock};

use domain::MediaId;
use futures::{SinkExt, StreamExt, channel::mpsc};
use tempfile::NamedTempFile;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    process::{Child, Command},
};
use tracing::{debug, warn};

use crate::{
    Client, Endpoint, Error, PlayerEvent, Result, Retry, fresh_endpoint,
    ipc::{Request, run_ipc},
    sweep_stale,
};

const EVENT_QUEUE: usize = 64;

/// The ipc folder is swept once, before the first session creates a socket.
static SWEPT: OnceLock<()> = OnceLock::new();

/// How to launch one player session.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct LaunchOptions {
    /// The app cache dir; its `ipc/` folder holds per-launch sockets.
    pub cache_dir: PathBuf,
    /// How long to wait for the player to open its IPC endpoint.
    pub ipc_retry: Retry,
    /// The media this session plays, for the engine's event routing.
    pub media: Option<MediaId>,
}

impl LaunchOptions {
    /// Standard options under `cache_dir`, with the normal mpv retry budget.
    pub fn new(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            cache_dir: cache_dir.into(),
            ipc_retry: Retry::mpv(),
            media: None,
        }
    }

    /// Options for IINA, whose embedded mpv takes longer to open IPC.
    pub fn iina(cache_dir: impl Into<PathBuf>) -> Self {
        Self {
            ipc_retry: Retry::iina(),
            ..Self::new(cache_dir)
        }
    }
}

/// What the supervisor does with the child when IPC closes first.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ChildPolicy {
    /// mpv: IPC closing means the session is over, so reap the child.
    KillOnIpcClose,
    /// IINA: closing the window leaves the app running; just end the session.
    LeaveOnIpcClose,
}

/// A running, supervised player session.
///
/// Dropping the session drops its event stream but does not kill the player:
/// it is detached and outlives the app. Commands take `&self` so the engine
/// can hold one handle and stream events from it.
#[derive(Debug)]
pub struct Session {
    client: Client,
    events: mpsc::Receiver<PlayerEvent>,
    endpoint: Endpoint,
    media: Option<MediaId>,
}

impl Session {
    /// The endpoint this launch listens on.
    #[must_use]
    pub fn endpoint(&self) -> &Endpoint {
        &self.endpoint
    }

    /// The media this session was launched for, when the engine supplied one.
    #[must_use]
    pub fn media(&self) -> Option<MediaId> {
        self.media
    }

    /// The typed event stream; use [`futures::StreamExt`] to drive it.
    pub fn events(&mut self) -> impl futures::Stream<Item = PlayerEvent> + '_ {
        &mut self.events
    }

    /// Waits for the next player event, or `None` when the session ends.
    pub async fn next_event(&mut self) -> Option<PlayerEvent> {
        self.events.next().await
    }

    /// Seeks to an absolute position in seconds.
    ///
    /// # Errors
    ///
    /// Returns an error if the session ended or the player rejected the seek.
    pub async fn seek(&self, seconds: f64) -> Result<()> {
        self.client
            .call(vec![
                serde_json::json!("seek"),
                serde_json::json!(seconds),
                serde_json::json!("absolute"),
            ])
            .await
            .map(|_| ())
    }

    /// Pauses playback.
    ///
    /// # Errors
    ///
    /// Returns an error if the session ended or the player rejected the command.
    pub async fn pause(&self) -> Result<()> {
        self.client
            .call(vec![
                serde_json::json!("set_property"),
                serde_json::json!("pause"),
                serde_json::json!(true),
            ])
            .await
            .map(|_| ())
    }

    /// Resumes playback.
    ///
    /// # Errors
    ///
    /// Returns an error if the session ended or the player rejected the command.
    pub async fn resume(&self) -> Result<()> {
        self.client
            .call(vec![
                serde_json::json!("set_property"),
                serde_json::json!("pause"),
                serde_json::json!(false),
            ])
            .await
            .map(|_| ())
    }

    /// Stops playback without closing the player.
    ///
    /// # Errors
    ///
    /// Returns an error if the session ended or the player rejected the command.
    pub async fn stop(&self) -> Result<()> {
        self.client
            .call(vec![serde_json::json!("stop")])
            .await
            .map(|_| ())
    }

    /// Replaces the current file with `url`.
    ///
    /// # Errors
    ///
    /// Returns an error if the session ended or the player rejected the command.
    pub async fn load_file(&self, url: &str) -> Result<()> {
        self.client
            .call(vec![
                serde_json::json!("loadfile"),
                serde_json::json!(url),
                serde_json::json!("replace"),
            ])
            .await
            .map(|_| ())
    }

    /// Shows a short message in the player's on-screen display.
    ///
    /// # Errors
    ///
    /// Returns an error if the session ended or the player rejected the command.
    pub async fn show_text(&self, text: &str) -> Result<()> {
        self.client
            .call(vec![
                serde_json::json!("show-text"),
                serde_json::json!(text),
            ])
            .await
            .map(|_| ())
    }

    /// Asks the player to quit, ending the supervised session.
    ///
    /// # Errors
    ///
    /// Returns an error if the session already ended or quit was rejected.
    pub async fn quit(&self) -> Result<()> {
        self.client
            .call(vec![serde_json::json!("quit")])
            .await
            .map(|_| ())
    }
}

/// Launches mpv on `file` and returns its supervised session.
///
/// The process is detached into its own group, its stdin is closed and its
/// stderr goes to a per-launch temp log. No resume position is passed on the
/// command line: resume happens after `MediaReady` (ticket #128). The ipc
/// folder is swept once, before the first socket is created.
///
/// # Errors
///
/// Returns an error if the endpoint, log or process cannot be created.
pub fn launch_mpv(
    executable: impl AsRef<std::path::Path>,
    file: &str,
    options: &LaunchOptions,
) -> Result<Session> {
    launch(
        executable,
        |endpoint| mpv_args(endpoint, file),
        options,
        ChildPolicy::KillOnIpcClose,
    )
}

/// Shared launch path for players that speak mpv's JSON IPC.
pub(crate) fn launch(
    executable: impl AsRef<std::path::Path>,
    args: impl FnOnce(&Endpoint) -> Vec<String>,
    options: &LaunchOptions,
    policy: ChildPolicy,
) -> Result<Session> {
    SWEPT.get_or_init(|| {
        if let Err(error) = sweep_stale(&options.cache_dir) {
            warn!("{error}");
        }
    });

    let executable = executable.as_ref();
    let endpoint = fresh_endpoint(&options.cache_dir)?;
    let log = tempfile::Builder::new()
        .prefix("autumn-mpv-")
        .suffix(".log")
        .tempfile()
        .map_err(|source| Error::Log { source })?;
    let stderr = log
        .as_file()
        .try_clone()
        .map_err(|source| Error::Log { source })?;

    let mut command = Command::new(executable);
    command
        .args(args(&endpoint))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    detach(&mut command);

    let child = command.spawn().map_err(|source| Error::Spawn {
        executable: executable.to_string_lossy().into_owned(),
        source,
    })?;

    let (command_tx, command_rx) = mpsc::unbounded::<Request>();
    let (event_tx, event_rx) = mpsc::channel::<PlayerEvent>(EVENT_QUEUE);

    tokio::spawn(supervise(
        child,
        endpoint.clone(),
        log,
        options.ipc_retry,
        policy,
        event_tx,
        command_rx,
    ));

    Ok(Session {
        client: Client::new(command_tx),
        events: event_rx,
        endpoint,
        media: options.media,
    })
}

/// The exact arguments mpv is launched with, kept pure for testing.
fn mpv_args(endpoint: &Endpoint, file: &str) -> Vec<String> {
    vec![
        "--force-window=immediate".to_owned(),
        "--keep-open=yes".to_owned(),
        format!("--input-ipc-server={endpoint}"),
        file.to_owned(),
    ]
}

#[cfg(unix)]
fn detach(command: &mut Command) {
    use std::os::unix::process::CommandExt;
    command.as_std_mut().process_group(0);
}

#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command
        .as_std_mut()
        .creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

async fn connect_with_retry(
    endpoint: &Endpoint,
    retry: Retry,
) -> Result<crate::endpoint::IpcStream> {
    let mut last = None;
    for attempt in 1..=retry.attempts {
        match endpoint.connect().await {
            Ok(stream) => return Ok(stream),
            Err(source) => {
                last = Some(source);
                if attempt < retry.attempts {
                    tokio::time::sleep(retry.delay).await;
                }
            }
        }
    }
    Err(Error::Connect {
        endpoint: endpoint.to_string(),
        attempts: retry.attempts,
        source: last.unwrap_or_else(|| std::io::Error::other("no connection attempts made")),
    })
}

/// Owns the child and the pump until the player or its IPC connection ends.
async fn supervise(
    child: Child,
    endpoint: Endpoint,
    _log: NamedTempFile,
    retry: Retry,
    policy: ChildPolicy,
    mut events: mpsc::Sender<PlayerEvent>,
    commands: mpsc::UnboundedReceiver<Request>,
) {
    let stream = match connect_with_retry(&endpoint, retry).await {
        Ok(stream) => stream,
        Err(error) => {
            warn!("{error}");
            let mut child = child;
            let _ = child.kill().await;
            let _ = endpoint.remove();
            let _ = events.send(PlayerEvent::Exited).await;
            return;
        }
    };

    supervise_connected(stream, child, endpoint, policy, events, commands).await;
}

/// Drives an already-connected session; tests inject a fake transport here.
pub(crate) async fn supervise_connected<S>(
    stream: S,
    mut child: Child,
    endpoint: Endpoint,
    policy: ChildPolicy,
    mut events: mpsc::Sender<PlayerEvent>,
    commands: mpsc::UnboundedReceiver<Request>,
) -> Child
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mut pump = tokio::spawn(run_ipc(stream, commands, events.clone()));
    let mut wait = Box::pin(child.wait());

    tokio::select! {
        result = &mut wait => {
            match result {
                Ok(status) => debug!("player exited with {status}"),
                Err(error) => warn!("could not reap player: {error}"),
            }
        }
        result = &mut pump => {
            match result {
                Ok(Ok(())) => debug!("player IPC closed"),
                Ok(Err(error)) => warn!("{error}"),
                Err(error) => warn!("player pump failed: {error}"),
            }
        }
    }

    pump.abort();
    drop(wait);
    if matches!(policy, ChildPolicy::KillOnIpcClose) {
        if child.try_wait().ok().flatten().is_none() {
            let _ = child.kill().await;
        }
        let _ = child.wait().await;
    }
    let _ = endpoint.remove();
    let _ = events.send(PlayerEvent::Exited).await;
    child
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[cfg(unix)]
    #[tokio::test]
    async fn connect_with_retry_waits_for_a_late_listener() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mpv-late.sock");
        let endpoint = crate::Endpoint::at(path.to_string_lossy().into_owned());

        let late = path.clone();
        let server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(120)).await;
            let listener = tokio::net::UnixListener::bind(late).unwrap();
            listener.accept().await.is_ok()
        });

        assert!(
            connect_with_retry(&endpoint, Retry::new(20, Duration::from_millis(20)))
                .await
                .is_ok()
        );
        assert!(server.await.unwrap());
    }

    #[test]
    fn mpv_launch_arguments_are_minimal_and_have_no_resume_position() {
        let cache = tempfile::tempdir().unwrap();
        let endpoint = fresh_endpoint(cache.path()).unwrap();
        let args = mpv_args(&endpoint, "file:///media/ep.mkv");

        assert_eq!(
            args,
            vec![
                "--force-window=immediate",
                "--keep-open=yes",
                &format!("--input-ipc-server={endpoint}"),
                "file:///media/ep.mkv",
            ]
        );
        assert!(
            args.iter().all(|arg| !arg.starts_with("--start=")),
            "resume is applied after media-ready, not on the command line"
        );
    }

    #[test]
    fn two_launches_use_different_ipc_arguments() {
        let cache = tempfile::tempdir().unwrap();
        let a = mpv_args(&fresh_endpoint(cache.path()).unwrap(), "a.mkv");
        let b = mpv_args(&fresh_endpoint(cache.path()).unwrap(), "b.mkv");
        assert_ne!(a.get(2), b.get(2), "each launch needs its own IPC endpoint");
    }
}
