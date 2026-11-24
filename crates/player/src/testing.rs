//! A fake mpv IPC server used by the crate's unit tests.

use futures::channel::mpsc;
use serde_json::{Value, json};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream, ReadHalf, WriteHalf},
    task::JoinHandle,
};

use crate::{
    Client, PlayerEvent, Result,
    ipc::{OBSERVED, run_ipc},
};

/// The server end of a fake IPC connection, driven directly from a test.
pub(crate) struct FakeMpv {
    reader: BufReader<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
}

impl FakeMpv {
    /// Starts a pump on a duplex pair and returns both handles.
    pub(crate) fn spawn() -> (
        Client,
        mpsc::Receiver<PlayerEvent>,
        JoinHandle<Result<()>>,
        Self,
    ) {
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, event_rx) = mpsc::channel(64);
        let pump = tokio::spawn(run_ipc(client_stream, command_rx, event_tx));
        let (read, write) = tokio::io::split(server_stream);
        (
            Client::new(command_tx),
            event_rx,
            pump,
            Self {
                reader: BufReader::new(read),
                writer: write,
            },
        )
    }

    /// Wraps an existing server stream, for tests that own the client end.
    pub(crate) fn from_stream(server_stream: DuplexStream) -> Self {
        let (read, write) = tokio::io::split(server_stream);
        Self {
            reader: BufReader::new(read),
            writer: write,
        }
    }

    /// Reads the observation commands the pump sends when it starts.
    pub(crate) async fn drain_observations(&mut self) -> Vec<Value> {
        let mut seen = Vec::with_capacity(OBSERVED.len());
        for _ in 0..OBSERVED.len() {
            seen.push(self.next_command().await);
        }
        seen
    }

    /// Reads one command line sent by the pump.
    pub(crate) async fn next_command(&mut self) -> Value {
        let mut line = String::new();
        self.reader
            .read_line(&mut line)
            .await
            .expect("fake server must read a command");
        serde_json::from_str(line.trim()).expect("command must be JSON")
    }

    /// Writes one JSON line to the pump.
    pub(crate) async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).expect("event must be JSON");
        bytes.push(b'\n');
        self.writer
            .write_all(&bytes)
            .await
            .expect("fake server must write");
    }

    /// Sends a successful reply to a command's request id.
    pub(crate) async fn reply_to(&mut self, request_id: u64, data: Value) {
        self.send(json!({
            "error": "success",
            "data": data,
            "request_id": request_id,
        }))
        .await;
    }

    /// Sends a property-change event.
    pub(crate) async fn property(&mut self, name: &str, data: &Value) {
        self.send(json!({
            "event": "property-change",
            "name": name,
            "data": data,
        }))
        .await;
    }
}

#[cfg(test)]
mod pump_tests {
    use std::time::Duration;

    use futures::StreamExt;

    use super::*;
    use crate::PlayerEvent;

    #[tokio::test]
    async fn observe_commands_cover_every_property_we_need() {
        let (_client, _events, _pump, mut fake) = FakeMpv::spawn();
        let observed = fake.drain_observations().await;

        assert_eq!(observed.len(), OBSERVED.len());
        let names: Vec<_> = observed
            .iter()
            .map(|value| {
                value
                    .get("command")
                    .and_then(|command| command.get(2))
                    .and_then(Value::as_str)
                    .unwrap_or("")
            })
            .collect();
        assert_eq!(names, OBSERVED);
    }

    #[tokio::test]
    async fn property_events_reach_the_typed_stream() {
        let (_client, mut events, _pump, mut fake) = FakeMpv::spawn();
        fake.drain_observations().await;

        fake.property("path", &json!("/media/ep.mkv")).await;
        fake.send(json!({ "event": "file-loaded" })).await;
        fake.property("pause", &json!(true)).await;

        assert_eq!(
            events.next().await,
            Some(PlayerEvent::MediaReady {
                path: Some("/media/ep.mkv".to_owned()),
                duration: None,
            })
        );
        assert_eq!(events.next().await, Some(PlayerEvent::Paused(true)));
    }

    #[tokio::test]
    async fn command_replies_are_matched_back_to_the_caller() {
        let (client, _events, _pump, mut fake) = FakeMpv::spawn();
        fake.drain_observations().await;

        let call = tokio::spawn(async move {
            client
                .call(vec![json!("seek"), json!(12.5), json!("absolute")])
                .await
        });
        tokio::task::yield_now().await;

        let command = fake.next_command().await;
        let id = command.get("request_id").and_then(Value::as_u64).unwrap();
        let args = command.get("command").and_then(Value::as_array).unwrap();
        assert_eq!(args.first(), Some(&json!("seek")));
        assert_eq!(args.get(1), Some(&json!(12.5)));
        assert_eq!(args.get(2), Some(&json!("absolute")));

        fake.reply_to(id, json!(null)).await;
        assert_eq!(call.await.unwrap().unwrap(), json!(null));
    }

    #[tokio::test]
    async fn a_failed_command_reply_is_an_error() {
        let (client, _events, _pump, mut fake) = FakeMpv::spawn();
        fake.drain_observations().await;

        let call = tokio::spawn(async move {
            client
                .call(vec![json!("set_property"), json!("pause"), json!(true)])
                .await
        });
        tokio::task::yield_now().await;

        let command = fake.next_command().await;
        let id = command.get("request_id").and_then(Value::as_u64).unwrap();
        fake.send(json!({ "error": "property unavailable", "request_id": id }))
            .await;

        let result = call.await.unwrap();
        assert!(
            result.is_err(),
            "a rejected command must not look successful"
        );
    }

    #[tokio::test]
    async fn dropping_the_connection_ends_the_pump_and_stream() {
        let (_client, mut events, pump, fake) = FakeMpv::spawn();
        drop(fake);

        let result = tokio::time::timeout(Duration::from_secs(2), pump)
            .await
            .expect("the pump must stop when its connection drops");
        result
            .expect("the pump task must finish without panicking")
            .ok();

        let event = tokio::time::timeout(Duration::from_secs(1), events.next())
            .await
            .expect("the event stream must close");
        assert_eq!(event, None);
    }

    #[tokio::test]
    async fn dropping_the_local_handles_keeps_the_detached_player_running() {
        let (client, events, pump, mut fake) = FakeMpv::spawn();
        fake.drain_observations().await;
        drop(client);
        drop(events);

        let still_running = tokio::time::timeout(Duration::from_millis(500), pump)
            .await
            .is_err();
        assert!(
            still_running,
            "the pump must outlive a dropped Session handle"
        );
    }

    #[tokio::test]
    async fn pending_commands_are_resolved_when_the_transport_closes() {
        let (client, _events, pump, mut fake) = FakeMpv::spawn();
        fake.drain_observations().await;

        let call = tokio::spawn(async move { client.call(vec![json!("quit")]).await });
        tokio::task::yield_now().await;
        let _quit = fake.next_command().await;
        drop(fake);

        let result = tokio::time::timeout(Duration::from_secs(2), call)
            .await
            .expect("the caller must be resolved when IPC closes")
            .expect("the call task must not panic");
        assert!(
            matches!(result, Err(crate::Error::Closed)),
            "an unanswered command must fail with Closed, not hang"
        );
        let _ = tokio::time::timeout(Duration::from_secs(2), pump).await;
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn property_events_flow_over_a_real_unix_socket() {
        use tokio::io::AsyncBufReadExt;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mpv-fake.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();

        let (command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let pump = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            crate::ipc::run_ipc(stream, command_rx, event_tx).await
        });

        let endpoint = crate::Endpoint::at(path.to_string_lossy().into_owned());
        let stream = endpoint.connect().await.unwrap();
        let _keepalive = command_tx;
        let (read, mut write) = tokio::io::split(stream);
        let mut reader = tokio::io::BufReader::new(read);

        for _ in 0..OBSERVED.len() {
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
        }
        for value in [
            json!({"event": "property-change", "name": "path", "data": "/media/ep.mkv"}),
            json!({"event": "file-loaded"}),
        ] {
            let mut bytes = serde_json::to_vec(&value).unwrap();
            bytes.push(b'\n');
            write.write_all(&bytes).await.unwrap();
        }

        assert_eq!(
            event_rx.next().await,
            Some(PlayerEvent::MediaReady {
                path: Some("/media/ep.mkv".to_owned()),
                duration: None,
            })
        );
        pump.abort();
    }

    #[tokio::test]
    async fn rapid_positions_are_coalesced_to_the_latest_value() {
        let (_client, mut events, _pump, mut fake) = FakeMpv::spawn();
        fake.drain_observations().await;

        fake.property("time-pos", &json!(1.0)).await;
        fake.property("time-pos", &json!(2.0)).await;
        fake.property("time-pos", &json!(3.0)).await;

        assert_eq!(
            events.next().await,
            Some(PlayerEvent::Position { seconds: 1.0 })
        );
        let quiet = tokio::time::timeout(Duration::from_millis(120), events.next()).await;
        assert!(quiet.is_err(), "positions must not arrive faster than 4 Hz");

        let later = tokio::time::timeout(Duration::from_millis(800), events.next())
            .await
            .expect("the newest position is emitted on the next tick")
            .expect("a position event");
        assert_eq!(later, PlayerEvent::Position { seconds: 3.0 });
    }
}
