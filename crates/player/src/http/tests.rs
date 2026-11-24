//! Recorded-response tests for the HTTP players.

use std::{
    sync::{Arc, Mutex},
    time::Duration,
};

use axum::{
    Json, Router,
    extract::State,
    http::Uri,
    response::{Html, IntoResponse},
    routing::get,
};
use futures::StreamExt;
use serde_json::{Value, json};
use tokio::{net::TcpListener, time::timeout};

use super::*;
use crate::PlayerEvent;

type Shared = (Arc<Mutex<Value>>, Arc<Mutex<Vec<String>>>);
type Recorded = (String, Arc<Mutex<Vec<String>>>);

async fn serve_vlc(shared: &Shared) -> String {
    let (body, hits) = shared.clone();
    let app = Router::new()
        .route(
            "/requests/status.json",
            get(
                move |State(hits): State<Arc<Mutex<Vec<String>>>>, uri: Uri| {
                    let body = Arc::clone(&body);
                    async move {
                        hits.lock().unwrap().push(uri.to_string());
                        Json(body.lock().unwrap().clone()).into_response()
                    }
                },
            ),
        )
        .with_state(Arc::clone(&hits));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    format!("http://{address}")
}

async fn serve_mpchc(html: &str) -> Recorded {
    let page = html.to_owned();
    let hits = Arc::new(Mutex::new(Vec::new()));
    let page_hits = Arc::clone(&hits);
    let app = Router::new()
        .route(
            "/variables.html",
            get(move || {
                let page = page.clone();
                async move { Html(page).into_response() }
            }),
        )
        .route(
            "/browser.html",
            get(move |uri: Uri| {
                let hits = Arc::clone(&page_hits);
                async move {
                    hits.lock().unwrap().push(uri.to_string());
                    Html("ok")
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), hits)
}

fn control_at(base: String, player: Player) -> Control {
    Control {
        base,
        password: None,
        player,
        http: client().unwrap(),
    }
}

#[test]
fn two_vlc_launches_get_different_ports_and_passwords() {
    let (a, control_a, _) = vlc_config("a.mkv").unwrap();
    let (b, control_b, _) = vlc_config("b.mkv").unwrap();

    assert_ne!(
        control_a.base, control_b.base,
        "each launch needs its own port"
    );
    assert_ne!(a.get(3), b.get(3), "each launch needs its own password");
    assert!(a.iter().any(|arg| arg.starts_with("--extraintf=http")));
}

#[test]
fn vlc_status_json_decodes_playing_and_stopped() {
    let playing = parse_vlc(r#"{"state":"playing","length":120.5,"time":3.5}"#).unwrap();
    assert!(playing.playing);
    assert!(!playing.stopped);
    assert_eq!(playing.length, 120.5);
    assert_eq!(playing.position, 3.5);

    let stopped = parse_vlc(r#"{"state":"stopped","length":0,"time":0}"#).unwrap();
    assert!(stopped.stopped);
}

#[test]
fn mpchc_variables_decode_playing_and_stopped() {
    let playing = parse_mpchc(
        r#"<html><p id="state">2</p><p id="position">3500</p>
           <p id="duration">120500</p><p id="file">ep.mkv</p></html>"#,
    );
    assert!(playing.playing);
    assert_eq!(playing.length, 120.5);
    assert_eq!(playing.position, 3.5);
    assert_eq!(playing.path.as_deref(), Some("ep.mkv"));

    let stopped = parse_mpchc(r#"<p id="state">0</p><p id="duration">0</p>"#);
    assert!(stopped.stopped);
}

#[tokio::test]
async fn recorded_vlc_status_is_fetched_and_seek_is_sent() {
    let shared = (
        Arc::new(Mutex::new(
            json!({"state":"playing","length":10.0,"time":1.0}),
        )),
        Arc::new(Mutex::new(Vec::new())),
    );
    let base = serve_vlc(&shared).await;
    let control = control_at(base, Player::Vlc);

    let status = control.status().await.unwrap();
    assert!(status.playing);
    assert_eq!(status.length, 10.0);

    dispatch(
        &control,
        &[json!("seek"), json!(2.5), json!("absolute")],
        &Signals::default(),
    )
    .await
    .unwrap();
    let hits = shared.1.lock().unwrap();
    assert!(
        hits.iter()
            .any(|uri| uri.contains("command=seek") && uri.contains("val=00%3A00%3A02")),
        "VLC must receive a whole-second clock, not a fractional value"
    );
}

#[tokio::test]
async fn a_polled_player_publishes_ready_position_and_eof() {
    let shared = (
        Arc::new(Mutex::new(
            json!({"state":"playing","length":10.0,"time":1.0}),
        )),
        Arc::new(Mutex::new(Vec::new())),
    );
    let base = serve_vlc(&shared).await;
    let (events_tx, events_rx) = mpsc::channel(16);
    let mut events_rx = Box::pin(events_rx);
    let signals = Signals::default();
    let task = tokio::spawn(poll(
        control_at(base, Player::Vlc),
        events_tx.clone(),
        signals.clone(),
    ));

    let ready = timeout(Duration::from_secs(2), events_rx.next())
        .await
        .unwrap();
    assert_eq!(
        ready,
        Some(PlayerEvent::MediaReady {
            path: None,
            duration: Some(10.0),
        })
    );
    let position = timeout(Duration::from_secs(2), events_rx.next())
        .await
        .unwrap();
    assert_eq!(position, Some(PlayerEvent::Position { seconds: 1.0 }));

    *shared.0.lock().unwrap() = json!({"state":"stopped","length":10.0,"time":10.0});
    let eof = timeout(Duration::from_secs(2), events_rx.next())
        .await
        .unwrap();
    assert_eq!(eof, Some(PlayerEvent::Eof));

    drop(events_tx);
    task.abort();
}

#[tokio::test]
async fn an_intentional_stop_does_not_look_like_eof() {
    let shared = (
        Arc::new(Mutex::new(
            json!({"state":"playing","length":10.0,"time":1.0}),
        )),
        Arc::new(Mutex::new(Vec::new())),
    );
    let base = serve_vlc(&shared).await;
    let control = control_at(base, Player::Vlc);
    let signals = Signals::default();
    let (events_tx, mut events_rx) = mpsc::channel(16);
    let poller = tokio::spawn(poll(control.clone(), events_tx, signals.clone()));

    assert!(
        timeout(Duration::from_secs(2), events_rx.next())
            .await
            .unwrap()
            .is_some()
    );
    assert!(
        timeout(Duration::from_secs(2), events_rx.next())
            .await
            .unwrap()
            .is_some()
    );

    dispatch(&control, &[json!("stop")], &signals)
        .await
        .unwrap();
    *shared.0.lock().unwrap() = json!({"state":"stopped","length":10.0,"time":1.0});

    let quiet = timeout(Duration::from_millis(600), events_rx.next()).await;
    assert!(quiet.is_err(), "a user-initiated stop must not emit Eof");
    poller.abort();
}

#[tokio::test]
async fn recorded_mpc_hc_variables_are_fetched_over_http() {
    let (base, _hits) = serve_mpchc(
        r#"<html><p id="state">2</p><p id="position">4000</p>
           <p id="duration">90000</p><p id="file">ep.mkv</p></html>"#,
    )
    .await;

    let status = control_at(base, Player::MpcHc).status().await.unwrap();

    assert!(status.playing);
    assert_eq!(status.length, 90.0);
    assert_eq!(status.path.as_deref(), Some("ep.mkv"));
}

#[cfg(unix)]
#[tokio::test]
async fn a_mpc_hc_that_never_opens_its_interface_is_a_typed_error() {
    let port = free_port().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let result = launch_mpchc(
        "/bin/sleep",
        "127.0.0.1",
        port,
        "30",
        &LaunchOptions::new(dir.path()),
    )
    .await;

    assert!(matches!(result, Err(Error::WebInterfaceDisabled)));
}

#[tokio::test]
async fn an_already_running_mpc_hc_is_reused() {
    let (base, hits) = serve_mpchc(
        r#"<html><p id="state">2</p><p id="position">1000</p>
           <p id="duration">60000</p></html>"#,
    )
    .await;
    let address: Vec<&str> = base.trim_start_matches("http://").split(':').collect();
    let host = address.first().copied().unwrap_or("127.0.0.1");
    let port = address
        .get(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap();
    let dir = tempfile::tempdir().unwrap();

    let session = launch_mpchc(
        "/nonexistent/mpc-hc.exe",
        host,
        port,
        "ep.mkv",
        &LaunchOptions::new(dir.path()),
    )
    .await
    .expect("a reachable web interface must be reused without spawning");

    assert!(session.endpoint().as_str().contains(&port.to_string()));
    assert!(
        hits.lock()
            .unwrap()
            .iter()
            .any(|uri| uri.contains("/browser.html") && uri.contains("path=ep.mkv")),
        "a reused instance must still load the requested file"
    );
}
