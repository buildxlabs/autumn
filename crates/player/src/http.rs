//! VLC HTTP and MPC-HC web-interface players behind mpv's event stream.

use std::{
    net::TcpListener,
    path::Path,
    process::Stdio,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use futures::{SinkExt, StreamExt, channel::mpsc};
use rand::random;
use serde::Deserialize;
use serde_json::Value;
use tempfile::NamedTempFile;
use tokio::process::{Child, Command};
use tracing::{debug, warn};

use crate::{
    Client, Endpoint, Error, LaunchOptions, PlayerEvent, Result, Session,
    ipc::Request,
    mpv::{detach, sweep_ipc_once},
};

/// The port MPC-HC's web interface uses when settings do not override it.
pub const DEFAULT_MPCHC_PORT: u16 = 13579;

const POLL_INTERVAL: Duration = Duration::from_millis(250);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
const MAX_POLL_FAILURES: u32 = 8;
const EVENT_QUEUE: usize = 64;
const READY_ATTEMPTS: u32 = 20;
const READY_DELAY: Duration = Duration::from_millis(200);

/// Flags shared by the command pump and the status poller.
#[derive(Debug, Clone, Default)]
struct Signals {
    quit: Arc<AtomicBool>,
    stopping: Arc<AtomicBool>,
}

/// Which web interface a control talks to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Player {
    Vlc,
    MpcHc,
}

#[derive(Debug, Clone)]
struct Control {
    base: String,
    password: Option<String>,
    player: Player,
    http: reqwest::Client,
}

impl Control {
    fn vlc(port: u16, password: String) -> Result<Self> {
        Ok(Self {
            base: format!("http://127.0.0.1:{port}"),
            password: Some(password),
            player: Player::Vlc,
            http: client()?,
        })
    }

    fn mpchc(host: &str, port: u16) -> Result<Self> {
        Ok(Self {
            base: format!("http://{host}:{port}"),
            password: None,
            player: Player::MpcHc,
            http: client()?,
        })
    }

    async fn get(&self, path: &str, query: &[(&str, &str)]) -> Result<String> {
        let mut url =
            reqwest::Url::parse(&format!("{}{path}", self.base)).map_err(|source| Error::Http {
                message: source.to_string(),
            })?;
        url.query_pairs_mut().extend_pairs(query.iter().copied());
        let mut request = self.http.get(url);
        if let Some(password) = &self.password {
            request = request.basic_auth("", Some(password));
        }
        let response = request.send().await.map_err(|source| Error::Http {
            message: source.to_string(),
        })?;
        if !response.status().is_success() {
            return Err(Error::Http {
                message: format!("HTTP {}", response.status().as_u16()),
            });
        }
        response.text().await.map_err(|source| Error::Http {
            message: source.to_string(),
        })
    }

    async fn status(&self) -> Result<Status> {
        let body = match self.player {
            Player::Vlc => self.get("/requests/status.json", &[]).await?,
            Player::MpcHc => self.get("/variables.html", &[]).await?,
        };
        match self.player {
            Player::Vlc => parse_vlc(&body),
            Player::MpcHc => Ok(parse_mpchc(&body)),
        }
    }
}

fn client() -> Result<reqwest::Client> {
    static INSTALL_TLS: std::sync::Once = std::sync::Once::new();
    INSTALL_TLS.call_once(|| {
        if let Err(already) = rustls::crypto::ring::default_provider().install_default() {
            debug!(?already, "rustls crypto provider already installed");
        }
    });
    reqwest::Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .build()
        .map_err(|source| Error::Http {
            message: source.to_string(),
        })
}

#[derive(Debug, Default, PartialEq)]
struct Status {
    playing: bool,
    paused: bool,
    stopped: bool,
    length: f64,
    position: f64,
    path: Option<String>,
}

#[derive(Debug, Deserialize)]
struct VlcStatus {
    #[serde(default)]
    state: String,
    #[serde(default)]
    length: f64,
    #[serde(default)]
    time: f64,
}

fn parse_vlc(body: &str) -> Result<Status> {
    let status: VlcStatus = serde_json::from_str(body).map_err(|source| Error::Http {
        message: format!("bad VLC status: {source}"),
    })?;
    Ok(Status {
        playing: status.state == "playing",
        paused: status.state == "paused",
        stopped: status.state == "stopped",
        length: status.length,
        position: status.time,
        path: None,
    })
}

/// Reads a `<p id="...">value</p>` field from MPC-HC's variables page.
fn html_field(html: &str, id: &str) -> Option<String> {
    let marker = format!("id=\"{id}\"");
    let at = html.find(&marker)?;
    let after_id = html.get(at + marker.len()..)?;
    let gt = after_id.find('>')?;
    let body = after_id.get(gt + 1..)?;
    let end = body.find("</p>")?;
    body.get(..end).map(str::trim).map(str::to_owned)
}

fn parse_mpchc(body: &str) -> Status {
    let state = html_field(body, "state")
        .and_then(|value| value.parse::<i32>().ok())
        .unwrap_or(0);
    // MPC-HC reports position and duration in milliseconds, while the rest
    // of the event stream speaks seconds.
    let position = html_field(body, "position")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0)
        / 1000.0;
    let length = html_field(body, "duration")
        .and_then(|value| value.parse::<f64>().ok())
        .unwrap_or(0.0)
        / 1000.0;
    Status {
        playing: state == 2,
        paused: state == 1,
        stopped: state == 0,
        length,
        position,
        path: html_field(body, "file"),
    }
}

/// Polls status at four hertz and publishes the same events as mpv IPC.
async fn poll(control: Control, mut events: mpsc::Sender<PlayerEvent>, signals: Signals) {
    let mut ticker = tokio::time::interval(POLL_INTERVAL);
    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    ticker.tick().await;
    let mut ready = false;
    let mut paused = false;
    let mut last_position = -1.0;
    let mut failures = 0;

    loop {
        ticker.tick().await;
        if signals.quit.load(Ordering::Relaxed) {
            break;
        }
        let status = match control.status().await {
            Ok(status) => status,
            Err(error) => {
                failures += 1;
                warn!("player status poll failed: {error}");
                if failures >= MAX_POLL_FAILURES {
                    break;
                }
                continue;
            }
        };
        failures = 0;

        if !ready && status.playing && status.length > 0.0 {
            ready = true;
            signals.stopping.store(false, Ordering::Relaxed);
            if events
                .send(PlayerEvent::MediaReady {
                    path: status.path.clone(),
                    duration: Some(status.length),
                })
                .await
                .is_err()
            {
                debug!("player event consumer went away; keeping the player alive");
            }
        }
        if ready && status.paused != paused {
            paused = status.paused;
            if events.send(PlayerEvent::Paused(paused)).await.is_err() {
                debug!("player event consumer went away; keeping the player alive");
            }
        }
        if ready && (status.stopped || (status.length > 0.0 && status.position >= status.length)) {
            ready = false;
            if signals.stopping.swap(false, Ordering::Relaxed) {
                continue;
            }
            if events.send(PlayerEvent::Eof).await.is_err() {
                debug!("player event consumer went away; keeping the player alive");
            }
            continue;
        }
        if ready && (status.position - last_position).abs() > f64::EPSILON {
            last_position = status.position;
            if events
                .send(PlayerEvent::Position {
                    seconds: status.position,
                })
                .await
                .is_err()
            {
                debug!("player event consumer went away; keeping the player alive");
            }
        }
    }
}

/// Turns mpv-shaped commands into web-interface requests.
async fn dispatch(control: &Control, command: &[Value], signals: &Signals) -> Result<Value> {
    let name = command.first().and_then(Value::as_str).unwrap_or_default();
    match name {
        "seek" => {
            let seconds = command.get(1).and_then(Value::as_f64).unwrap_or(0.0);
            match control.player {
                Player::Vlc => {
                    // VLC parses `val` with a whole-seconds pattern: `0.25`
                    // would be read as 25 seconds. Send an HH:MM:SS clock.
                    control
                        .get(
                            "/requests/status.json",
                            &[("command", "seek"), ("val", &clock(seconds))],
                        )
                        .await?;
                }
                Player::MpcHc => {
                    control
                        .get(
                            "/command.html",
                            &[("wm_command", "-1"), ("position", &clock(seconds))],
                        )
                        .await?;
                }
            }
            Ok(Value::Null)
        }
        "set_property" if command.get(1).and_then(Value::as_str) == Some("pause") => {
            let paused = command.get(2).and_then(Value::as_bool).unwrap_or(false);
            match control.player {
                Player::Vlc => {
                    let command = if paused {
                        "pl_forcepause"
                    } else {
                        "pl_forceresume"
                    };
                    control
                        .get("/requests/status.json", &[("command", command)])
                        .await?;
                }
                Player::MpcHc => {
                    let code = if paused { "888" } else { "887" };
                    control
                        .get("/command.html", &[("wm_command", code)])
                        .await?;
                }
            }
            Ok(Value::Null)
        }
        "stop" => {
            signals.stopping.store(true, Ordering::Relaxed);
            let (path, query): (&str, &[(&str, &str)]) = match control.player {
                Player::Vlc => ("/requests/status.json", &[("command", "pl_stop")]),
                Player::MpcHc => ("/command.html", &[("wm_command", "890")]),
            };
            control.get(path, query).await?;
            Ok(Value::Null)
        }
        "loadfile" => {
            let url = command.get(1).and_then(Value::as_str).unwrap_or_default();
            match control.player {
                Player::Vlc => {
                    control
                        .get(
                            "/requests/status.json",
                            &[("command", "in_play"), ("input", url)],
                        )
                        .await?;
                }
                Player::MpcHc => {
                    control.get("/browser.html", &[("path", url)]).await?;
                }
            }
            Ok(Value::Null)
        }
        "quit" => {
            signals.quit.store(true, Ordering::Relaxed);
            signals.stopping.store(true, Ordering::Relaxed);
            let (path, query): (&str, &[(&str, &str)]) = match control.player {
                Player::Vlc => ("/requests/status.json", &[("command", "pl_stop")]),
                Player::MpcHc => ("/command.html", &[("wm_command", "816")]),
            };
            control.get(path, query).await?;
            Ok(Value::Null)
        }
        "show-text" => Ok(Value::Null),
        other => Err(Error::Http {
            message: format!("unsupported player command: {other}"),
        }),
    }
}

async fn commands(
    control: Control,
    mut requests: mpsc::UnboundedReceiver<Request>,
    signals: Signals,
) {
    while let Some(request) = requests.next().await {
        let result = dispatch(&control, &request.command, &signals).await;
        if request.reply.send(result).is_err() {
            debug!("player command caller went away");
        }
    }
}

#[expect(clippy::as_conversions, reason = "MPC-HC takes a wall-clock string")]
#[expect(
    clippy::cast_possible_truncation,
    reason = "MPC-HC seeks take a whole-second wall-clock string"
)]
#[expect(
    clippy::cast_sign_loss,
    reason = "negative seek positions are clamped to zero"
)]
fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    format!(
        "{:02}:{:02}:{:02}",
        total / 3600,
        (total / 60) % 60,
        total % 60
    )
}

fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").map_err(|source| Error::Http {
        message: source.to_string(),
    })?;
    let address = listener.local_addr().map_err(|source| Error::Http {
        message: source.to_string(),
    })?;
    Ok(address.port())
}

fn password() -> String {
    format!("{:032x}{:032x}", random::<u128>(), random::<u128>())
}

/// Launches VLC with a fresh HTTP interface and returns its session.
///
/// # Errors
///
/// Returns an error if the port, log, process or HTTP client cannot be
/// created, or if the interface never becomes ready.
pub async fn launch_vlc(
    executable: impl AsRef<Path>,
    file: &str,
    options: &LaunchOptions,
) -> Result<Session> {
    let (args, control, endpoint) = vlc_config(file)?;
    let (child, log) = spawn(executable, args, options)?;
    if let Err(error) = wait_ready(&control, "/requests/status.json").await {
        kill(child).await;
        return Err(error);
    }
    Ok(finish(Some(child), log, control, options, endpoint))
}

/// Builds a fresh VLC HTTP configuration, kept pure for testing.
fn vlc_config(file: &str) -> Result<(Vec<String>, Control, Endpoint)> {
    let port = free_port()?;
    let secret = password();
    let control = Control::vlc(port, secret.clone())?;
    let endpoint = Endpoint::http(control.base.clone());
    let args = vec![
        "--extraintf=http".to_owned(),
        "--http-host=127.0.0.1".to_owned(),
        format!("--http-port={port}"),
        format!("--http-password={secret}"),
        file.to_owned(),
    ];
    Ok((args, control, endpoint))
}

/// Launches MPC-HC and attaches to its web interface.
///
/// An already running instance with the interface enabled is reused;
/// otherwise the executable is spawned and the interface is awaited.
///
/// # Errors
///
/// Returns [`Error::WebInterfaceDisabled`] if a fresh instance never opens
/// its web interface, or an error if the process cannot be started.
pub async fn launch_mpchc(
    executable: impl AsRef<Path>,
    host: &str,
    port: u16,
    file: &str,
    options: &LaunchOptions,
) -> Result<Session> {
    let control = Control::mpchc(host, port)?;
    let endpoint = Endpoint::http(control.base.clone());

    if control.get("/variables.html", &[]).await.is_ok() {
        // An existing instance must still play the requested file.
        dispatch(
            &control,
            &[serde_json::json!("loadfile"), serde_json::json!(file)],
            &Signals::default(),
        )
        .await?;
        let log = tempfile::Builder::new()
            .tempfile()
            .map_err(|source| Error::Log { source })?;
        return Ok(finish(None, log, control, options, endpoint));
    }

    let (child, log) = spawn(executable, vec![file.to_owned()], options)?;
    if wait_ready(&control, "/variables.html").await.is_err() {
        kill(child).await;
        return Err(Error::WebInterfaceDisabled);
    }
    Ok(finish(Some(child), log, control, options, endpoint))
}

async fn wait_ready(control: &Control, path: &str) -> Result<()> {
    for _ in 0..READY_ATTEMPTS {
        if control.get(path, &[]).await.is_ok() {
            return Ok(());
        }
        tokio::time::sleep(READY_DELAY).await;
    }
    Err(Error::Http {
        message: format!("{path} never became ready"),
    })
}

fn spawn(
    executable: impl AsRef<Path>,
    args: Vec<String>,
    options: &LaunchOptions,
) -> Result<(Child, NamedTempFile)> {
    sweep_ipc_once(&options.cache_dir);

    let log = tempfile::Builder::new()
        .prefix("autumn-player-")
        .suffix(".log")
        .tempfile()
        .map_err(|source| Error::Log { source })?;
    let stderr = log
        .as_file()
        .try_clone()
        .map_err(|source| Error::Log { source })?;

    let mut command = Command::new(executable.as_ref());
    command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::from(stderr));
    detach(&mut command);
    let child = command.spawn().map_err(|source| Error::Spawn {
        executable: executable.as_ref().to_string_lossy().into_owned(),
        source,
    })?;
    Ok((child, log))
}

fn finish(
    child: Option<Child>,
    log: NamedTempFile,
    control: Control,
    options: &LaunchOptions,
    endpoint: Endpoint,
) -> Session {
    let (command_tx, command_rx) = mpsc::unbounded::<Request>();
    let (event_tx, event_rx) = mpsc::channel::<PlayerEvent>(EVENT_QUEUE);
    tokio::spawn(supervise(child, control, log, event_tx, command_rx));

    Session::new(Client::new(command_tx), event_rx, endpoint, options.media)
}

async fn kill(mut child: Child) {
    if child.try_wait().ok().flatten().is_none() {
        let _ = child.kill().await;
    }
    let _ = child.wait().await;
}

async fn supervise(
    child: Option<Child>,
    control: Control,
    _log: NamedTempFile,
    mut events: mpsc::Sender<PlayerEvent>,
    requests: mpsc::UnboundedReceiver<Request>,
) {
    let signals = Signals::default();
    let mut poller = tokio::spawn(poll(control.clone(), events.clone(), signals.clone()));
    let commander = tokio::spawn(commands(control, requests, signals));

    // Reap promptly when the player exits, instead of waiting for the poller
    // to time out repeatedly. A poller that ends on its own (quit, EOF or a
    // lost interface) still owns the session, so stop a child that survives.
    if let Some(mut child) = child {
        let poller_ended = {
            let mut wait = Box::pin(child.wait());
            tokio::select! {
                _ = &mut wait => false,
                _ = &mut poller => true,
            }
        };
        if poller_ended {
            kill(child).await;
        }
    } else {
        let _ = (&mut poller).await;
    }
    commander.abort();
    if !poller.is_finished() {
        poller.abort();
    }
    let _ = events.send(PlayerEvent::Exited).await;
}

#[cfg(test)]
mod tests;
