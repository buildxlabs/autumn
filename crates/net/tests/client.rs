//! Integration tests for the shared client, using a loopback mock server only.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{
    Router,
    extract::State,
    http::{HeaderName, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use net::{Client, Error, HostKind, Method, Request};
use tokio::{net::TcpListener, task::JoinHandle, time::Instant};

#[derive(Debug, Default)]
struct MockState {
    hits: AtomicUsize,
}

impl MockState {
    fn record(&self) {
        self.hits.fetch_add(1, Ordering::SeqCst);
    }

    fn count(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

async fn ok(State(state): State<Arc<MockState>>) -> Response {
    state.record();
    (StatusCode::OK, "ok").into_response()
}

async fn server_error(State(state): State<Arc<MockState>>) -> Response {
    state.record();
    (StatusCode::INTERNAL_SERVER_ERROR, "boom").into_response()
}

async fn forbidden(State(state): State<Arc<MockState>>) -> Response {
    state.record();
    (StatusCode::FORBIDDEN, "no").into_response()
}

async fn rate_limited(State(state): State<Arc<MockState>>) -> Response {
    state.record();
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "1")],
        "slow down",
    )
        .into_response()
}

async fn rate_limited_with_later_reset(State(state): State<Arc<MockState>>) -> Response {
    state.record();
    let reset = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        + 3;
    (
        StatusCode::TOO_MANY_REQUESTS,
        [
            (header::RETRY_AFTER, "1".to_owned()),
            (
                HeaderName::from_static("x-ratelimit-reset"),
                reset.to_string(),
            ),
        ],
        "slow down",
    )
        .into_response()
}

async fn stall(State(state): State<Arc<MockState>>) -> Response {
    state.record();
    std::future::pending::<Response>().await
}

async fn rate_limited_then_ok(State(state): State<Arc<MockState>>) -> Response {
    if state.count() == 0 {
        rate_limited(State(state)).await
    } else {
        ok(State(state)).await
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a failed bind or serve cannot be recovered"
)]
async fn serve(app: Router) -> (String, JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), server)
}

fn get_request(url: &str) -> Request {
    Request::new(Method::Get, url)
}

fn post_request(url: &str) -> Request {
    Request::new(Method::Post, url)
}

#[tokio::test]
async fn bursts_are_spaced_by_the_token_bucket() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", get(ok))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;
    let url = format!("{base}/");

    // One token per second, two in the bucket: the third and fourth calls wait.
    let client = Client::builder()
        .host("127.0.0.1", 60, 2, HostKind::Generic)
        .build()
        .unwrap();

    let start = Instant::now();
    for _ in 0..4 {
        let response = client.read(get_request(&url)).await.unwrap();
        assert!(response.is_success());
    }
    let elapsed = start.elapsed();
    assert!(
        elapsed >= Duration::from_secs(2),
        "four calls with a two-token burst should take at least two seconds, took {elapsed:?}"
    );
    assert_eq!(state.count(), 4);
    server.abort();
}

#[tokio::test]
async fn reads_retry_three_times_then_fail() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", get(server_error))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .base_backoff(Duration::from_millis(10))
        .build()
        .unwrap();
    let error = client
        .read(get_request(&format!("{base}/")))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::Http { status, .. } if status == 500),
        "expected the last HTTP error, got {error:?}"
    );
    assert_eq!(state.count(), 4, "one attempt plus three retries");
    server.abort();
}

#[tokio::test]
async fn mutations_are_not_retried() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", post(server_error))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .base_backoff(Duration::from_millis(10))
        .build()
        .unwrap();
    let error = client
        .mutate(post_request(&format!("{base}/")))
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Http { .. }), "got {error:?}");
    assert_eq!(state.count(), 1, "a mutation must be sent exactly once");
    server.abort();
}

#[tokio::test]
async fn a_429_pauses_the_host_queue_then_resumes() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", get(rate_limited_then_ok))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .host("127.0.0.1", 120, 10, HostKind::Generic)
        .build()
        .unwrap();
    let start = Instant::now();
    let response = client.read(get_request(&format!("{base}/"))).await.unwrap();

    assert!(response.is_success());
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "the retry must wait out Retry-After"
    );
    assert!(client.lockout("127.0.0.1").await.is_none());
    assert_eq!(state.count(), 2, "one 429 then one successful retry");
    server.abort();
}

#[tokio::test]
async fn lockout_exposes_time_remaining() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", post(rate_limited))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .host("127.0.0.1", 120, 10, HostKind::Generic)
        .build()
        .unwrap();
    let error = client
        .mutate(post_request(&format!("{base}/")))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::RateLimited { retry_after, .. } if retry_after == Duration::from_secs(1)),
        "expected the advertised Retry-After, got {error:?}"
    );
    let lockout = client
        .lockout("127.0.0.1")
        .await
        .expect("the host is locked out");
    let remaining = lockout.remaining();
    assert!(
        remaining > Duration::ZERO,
        "countdown must still be running"
    );
    assert!(
        remaining <= Duration::from_secs(1),
        "countdown must not exceed Retry-After, got {remaining:?}"
    );
    server.abort();
}

#[tokio::test]
async fn an_anilist_403_maps_to_unavailable() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", get(forbidden))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .host("127.0.0.1", 120, 10, HostKind::AniList)
        .build()
        .unwrap();
    let error = client
        .read(get_request(&format!("{base}/")))
        .await
        .unwrap_err();

    assert!(matches!(error, Error::AniListUnavailable), "got {error:?}");
    assert_eq!(state.count(), 1, "a 403 outage must not be retried");
    server.abort();
}

#[tokio::test]
async fn the_later_of_retry_after_and_reset_is_honored() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", post(rate_limited_with_later_reset))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder().build().unwrap();
    let error = client
        .mutate(post_request(&format!("{base}/")))
        .await
        .unwrap_err();

    assert!(
        matches!(error, Error::RateLimited { retry_after, .. } if retry_after > Duration::from_secs(1) && retry_after <= Duration::from_secs(3)),
        "the later X-RateLimit-Reset must win, got {error:?}"
    );
    server.abort();
}

#[tokio::test]
async fn lockout_disappears_when_it_expires() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", post(rate_limited))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .host("127.0.0.1", 120, 10, HostKind::Generic)
        .build()
        .unwrap();
    let error = client
        .mutate(post_request(&format!("{base}/")))
        .await
        .unwrap_err();
    assert!(matches!(error, Error::RateLimited { .. }));

    tokio::time::sleep(Duration::from_secs(2)).await;
    assert!(
        client.lockout("127.0.0.1").await.is_none(),
        "an expired lockout must not show a frozen zero countdown"
    );
    server.abort();
}

#[tokio::test]
async fn a_stalled_read_times_out_retries_then_fails() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", get(stall))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder()
        .base_backoff(Duration::from_millis(10))
        .connect_timeout(Duration::from_millis(200))
        .request_timeout(Duration::from_millis(200))
        .build()
        .unwrap();
    let error = client
        .read(get_request(&format!("{base}/")))
        .await
        .unwrap_err();

    assert!(matches!(error, Error::Transport { .. }), "got {error:?}");
    assert_eq!(state.count(), 4, "one attempt plus three retries");
    server.abort();
}

#[tokio::test]
async fn a_429_on_an_unregistered_host_still_delays_the_retry() {
    let state = Arc::new(MockState::default());
    let app = Router::new()
        .route("/", get(rate_limited_then_ok))
        .with_state(Arc::clone(&state));
    let (base, server) = serve(app).await;

    let client = Client::builder().build().unwrap();
    let start = Instant::now();
    let response = client.read(get_request(&format!("{base}/"))).await.unwrap();

    assert!(response.is_success());
    assert!(
        start.elapsed() >= Duration::from_secs(1),
        "a host without a limiter must still wait out Retry-After"
    );
    server.abort();
}
