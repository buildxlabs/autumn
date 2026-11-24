//! The mpv JSON IPC pump: observe properties, translate events and reply to commands.

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use futures::{
    SinkExt, StreamExt,
    channel::{mpsc, oneshot},
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tracing::{debug, warn};

use crate::{
    Error, Result,
    event::{Chapter, PlayerEvent, PositionLimiter},
};

const POSITION_INTERVAL: Duration = Duration::from_millis(250);
pub(crate) const OBSERVED: &[&str] = &[
    "time-pos",
    "duration",
    "pause",
    "eof-reached",
    "path",
    "chapter",
    "chapter-list",
];

/// One command awaiting the player's reply.
#[derive(Debug)]
pub(crate) struct Request {
    pub(crate) command: Vec<Value>,
    pub(crate) reply: oneshot::Sender<Result<Value>>,
}

/// The command side of a running IPC session.
///
/// Cloning shares the one command channel; a clone stops working once the
/// session ends.
#[derive(Debug, Clone)]
pub struct Client {
    commands: mpsc::UnboundedSender<Request>,
}

impl Client {
    pub(crate) fn new(commands: mpsc::UnboundedSender<Request>) -> Self {
        Self { commands }
    }

    /// Sends an arbitrary mpv command and waits for its reply.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Closed`] if the session ended, or [`Error::Command`]
    /// if the player rejected the command.
    pub async fn call(&self, command: Vec<Value>) -> Result<Value> {
        let (reply, receiver) = oneshot::channel();
        self.commands
            .unbounded_send(Request { command, reply })
            .map_err(|_| Error::Closed)?;
        receiver.await.map_err(|_| Error::Closed)?
    }
}

#[derive(Debug)]
struct Pending {
    command: String,
    reply: oneshot::Sender<Result<Value>>,
}

#[derive(Debug, Default)]
struct State {
    path: Option<String>,
    duration: Option<f64>,
    chapter: Option<usize>,
    chapters: Vec<Chapter>,
    eof_emitted: bool,
    ready: bool,
}

/// What one IPC message turns into.
#[derive(Debug)]
enum Translation {
    /// Player events to publish.
    Events(Vec<PlayerEvent>),
    /// A command reply to route back to its caller.
    Reply {
        /// The `request_id` mpv echoed.
        id: u64,
        /// The command's outcome.
        result: Result<Value>,
    },
    /// An observation reply or an unknown message.
    Ignore,
}

/// Converts one JSON message into events or a reply, updating session state.
fn translate(
    value: &Value,
    state: &mut State,
    positions: &mut PositionLimiter,
    now: Instant,
) -> Translation {
    if let Some(event) = value.get("event").and_then(Value::as_str) {
        return match event {
            "property-change" => property_change(value, state, positions, now),
            "file-loaded" => {
                state.ready = true;
                state.eof_emitted = false;
                Translation::Events(vec![PlayerEvent::MediaReady {
                    path: state.path.clone(),
                    duration: state.duration,
                }])
            }
            "playback-restart" if !state.ready => {
                state.ready = true;
                Translation::Events(vec![PlayerEvent::MediaReady {
                    path: state.path.clone(),
                    duration: state.duration,
                }])
            }
            "end-file" => {
                let reason = value.get("reason").and_then(Value::as_str).unwrap_or("");
                if reason == "eof" && !state.eof_emitted {
                    state.eof_emitted = true;
                    Translation::Events(vec![PlayerEvent::Eof])
                } else {
                    Translation::Ignore
                }
            }
            _ => Translation::Ignore,
        };
    }

    let Some(error) = value.get("error").and_then(Value::as_str) else {
        return Translation::Ignore;
    };
    let Some(id) = value.get("request_id").and_then(Value::as_u64) else {
        return Translation::Ignore;
    };
    let result = if error == "success" {
        Ok(value.get("data").cloned().unwrap_or(Value::Null))
    } else {
        Err(Error::Command {
            command: format!("request {id}"),
            message: error.to_owned(),
        })
    };
    Translation::Reply { id, result }
}

fn property_change(
    value: &Value,
    state: &mut State,
    positions: &mut PositionLimiter,
    now: Instant,
) -> Translation {
    let Some(name) = value.get("name").and_then(Value::as_str) else {
        return Translation::Ignore;
    };
    let data = value.get("data");

    match name {
        "time-pos" => {
            if let Some(seconds) = data.and_then(Value::as_f64) {
                positions.push(seconds);
            }
            positions.take(now).map_or(Translation::Ignore, |seconds| {
                Translation::Events(vec![PlayerEvent::Position { seconds }])
            })
        }
        "duration" => {
            state.duration = data.and_then(Value::as_f64);
            Translation::Ignore
        }
        "path" => {
            state.path = data.and_then(Value::as_str).map(str::to_owned);
            Translation::Ignore
        }
        "pause" => data
            .and_then(Value::as_bool)
            .map_or(Translation::Ignore, |paused| {
                Translation::Events(vec![PlayerEvent::Paused(paused)])
            }),
        "eof-reached" => {
            let reached = data.and_then(Value::as_bool).unwrap_or(false);
            if reached && !state.eof_emitted {
                state.eof_emitted = true;
                Translation::Events(vec![PlayerEvent::Eof])
            } else {
                Translation::Ignore
            }
        }
        "chapter" => {
            state.chapter = data
                .and_then(Value::as_u64)
                .and_then(|index| usize::try_from(index).ok());
            Translation::Events(vec![PlayerEvent::Chapters {
                current: state.chapter,
                list: state.chapters.clone(),
            }])
        }
        "chapter-list" => {
            let list = data
                .and_then(Value::as_array)
                .and_then(|values| {
                    serde_json::from_value::<Vec<Chapter>>(Value::Array(values.clone())).ok()
                })
                .unwrap_or_default();
            state.chapters.clone_from(&list);
            Translation::Events(vec![PlayerEvent::Chapters {
                current: state.chapter,
                list,
            }])
        }
        _ => Translation::Ignore,
    }
}

async fn read_messages<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    messages: tokio::sync::mpsc::UnboundedSender<Value>,
) {
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let Ok(value) = serde_json::from_str(line.trim()) else {
            warn!("ignoring malformed player IPC line: {}", line.trim());
            continue;
        };
        if messages.send(value).is_err() {
            break;
        }
    }
}

/// Runs the IPC pump until the connection closes or the command channel drops.
///
/// The stream can be a real socket or, in tests, a duplex pair driven by a
/// fake mpv. Property events are translated into [`PlayerEvent`]s and
/// positions are coalesced to four per second.
///
/// # Errors
///
/// Returns [`Error::Ipc`] if writing a command to the player fails.
pub(crate) async fn run_ipc<S>(
    stream: S,
    mut commands: mpsc::UnboundedReceiver<Request>,
    mut events: mpsc::Sender<PlayerEvent>,
) -> Result<()>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (read, mut write) = tokio::io::split(stream);
    let mut reader = BufReader::new(read);
    let (messages_tx, mut messages_rx) = tokio::sync::mpsc::unbounded_channel();
    let reader_task = tokio::spawn(async move { read_messages(&mut reader, messages_tx).await });

    let mut next_id = 1_u64;
    for name in OBSERVED {
        let payload = json!({
            "command": ["observe_property", next_id, name],
            "request_id": next_id,
        });
        write_command(&mut write, &payload).await?;
        next_id += 1;
    }

    let mut state = State::default();
    let mut positions = PositionLimiter::new(POSITION_INTERVAL);
    let mut pending: HashMap<u64, Pending> = HashMap::new();
    let mut ticker = tokio::time::interval(POSITION_INTERVAL);
    ticker.tick().await;

    let outcome = 'pump: loop {
        tokio::select! {
            message = messages_rx.recv() => {
                let Some(value) = message else { break Ok(()); };
                match translate(&value, &mut state, &mut positions, Instant::now()) {
                    Translation::Events(events_to_send) => {
                        for event in events_to_send {
                            if events.send(event).await.is_err() {
                                break 'pump Ok(());
                            }
                        }
                    }
                    Translation::Reply { id, result } => {
                        if let Some(pending) = pending.remove(&id)
                            && pending.reply.send(result).is_err()
                        {
                            debug!("player command caller went away: {}", pending.command);
                        }
                    }
                    Translation::Ignore => {}
                }
            }
            request = commands.next() => {
                let Some(request) = request else { break Ok(()); };
                let id = next_id;
                next_id += 1;
                let command = format_command(&request.command);
                let payload = json!({ "command": request.command, "request_id": id });
                write_command(&mut write, &payload).await?;
                pending.insert(id, Pending { command, reply: request.reply });
            }
            _ = ticker.tick() => {
                if let Some(seconds) = positions.take(Instant::now())
                    && events.send(PlayerEvent::Position { seconds }).await.is_err()
                {
                    break Ok(());
                }
            }
        }
    };

    reader_task.abort();
    positions.clear();
    outcome
}

async fn write_command<W: AsyncWrite + Unpin>(write: &mut W, payload: &Value) -> Result<()> {
    let mut line =
        serde_json::to_vec(payload).map_err(|source| Error::Endpoint(source.to_string()))?;
    line.push(b'\n');
    write
        .write_all(&line)
        .await
        .map_err(|source| Error::Ipc { source })
}

fn format_command(command: &[Value]) -> String {
    command
        .iter()
        .map(|value| {
            value
                .as_str()
                .map_or_else(|| value.to_string(), str::to_owned)
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests;
