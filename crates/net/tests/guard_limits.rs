//! Caps, headers, cookies, retries and rate limits on extension requests (ADR 0015).
//!
//! Loopback servers only; `allow_loopback` and static DNS stand in for public hosts.

#![expect(clippy::unwrap_used, reason = "test harness panics on setup failure")]

use std::{
    io,
    net::{IpAddr, Ipv4Addr},
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

use axum::{
    Router,
    body::Body,
    extract::{Path, State},
    http::{HeaderMap, StatusCode, header},
    response::{AppendHeaders, IntoResponse, Response},
    routing::{get, post},
};
use net::{
    DeclaredHost, Error, ExtensionHosts, GuardedClient, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES,
    Method, Request,
};
use tokio::{net::TcpListener, task::JoinHandle};

#[derive(Debug, Clone, Default)]
struct Hits(Arc<AtomicUsize>);

impl Hits {
    fn record(&self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }

    fn count(&self) -> usize {
        self.0.load(Ordering::SeqCst)
    }
}

struct Server {
    base: String,
    hits: Hits,
    task: JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

async fn echo(State(hits): State<Hits>, headers: HeaderMap) -> String {
    hits.record();
    let mut lines: Vec<String> = headers
        .iter()
        .map(|(name, value)| format!("{name}: {}", value.to_str().unwrap_or("?")))
        .collect();
    lines.sort();
    lines.join("\n")
}

async fn cookie(State(hits): State<Hits>) -> Response {
    hits.record();
    (
        AppendHeaders([
            (header::SET_COOKIE, "session=abc; Path=/"),
            (header::SET_COOKIE, "theme=dark; Path=/"),
        ]),
        "ok",
    )
        .into_response()
}

/// `n` bytes in 64 KiB chunks, with no `Content-Length`.
async fn streamed(State(hits): State<Hits>, Path(n): Path<usize>) -> Response {
    hits.record();
    let chunks = (0..n.div_ceil(65_536)).map(move |index| {
        let size = (n - index * 65_536).min(65_536);
        Ok::<_, io::Error>(vec![b'x'; size])
    });
    Body::from_stream(futures::stream::iter(chunks)).into_response()
}

async fn sized(State(hits): State<Hits>, Path(n): Path<usize>) -> Vec<u8> {
    hits.record();
    vec![b'x'; n]
}

async fn unavailable(State(hits): State<Hits>) -> StatusCode {
    hits.record();
    StatusCode::SERVICE_UNAVAILABLE
}

async fn limited(State(hits): State<Hits>) -> Response {
    hits.record();
    (
        StatusCode::TOO_MANY_REQUESTS,
        [(header::RETRY_AFTER, "120")],
        "slow down",
    )
        .into_response()
}

async fn serve() -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let hits = Hits::default();
    let app = Router::new()
        .route("/echo", get(echo).post(echo))
        .route("/cookie", get(cookie))
        .route("/streamed/{n}", get(streamed))
        .route("/sized/{n}", get(sized))
        .route("/unavailable", get(unavailable))
        .route("/limited", get(limited))
        .route(
            "/upload",
            post(|body: axum::body::Bytes| async move { body.len().to_string() }),
        )
        .with_state(hits.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server {
        base: format!("http://api.test:{port}"),
        hits,
        task,
    }
}

fn client() -> GuardedClient {
    GuardedClient::builder()
        .allow_loopback()
        .static_dns([("api.test", vec![IpAddr::V4(Ipv4Addr::LOCALHOST)])])
        .build()
        .unwrap()
}

fn hosts(id: &str) -> ExtensionHosts {
    ExtensionHosts::new(id, vec![DeclaredHost::parse("api.test").unwrap()])
}

fn get_request(server: &Server, path: &str) -> Request {
    Request::new(Method::Get, format!("{}{path}", server.base))
}

#[tokio::test]
async fn a_streamed_body_over_the_cap_aborts_with_too_large() {
    let server = serve().await;
    let client = client();
    let over = usize::try_from(MAX_RESPONSE_BYTES).unwrap() + 1;

    let response = client
        .send(
            &hosts("ext"),
            get_request(&server, &format!("/streamed/{over}")),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.bytes().await,
        Err(Error::ResponseTooLarge { limit, .. }) if limit == MAX_RESPONSE_BYTES
    ));

    let at_cap = usize::try_from(MAX_RESPONSE_BYTES).unwrap();
    let body = client
        .send(
            &hosts("ext"),
            get_request(&server, &format!("/streamed/{at_cap}")),
        )
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(body.len(), at_cap);
}

#[tokio::test]
async fn a_caller_can_set_a_smaller_cap_but_not_a_larger_one() {
    let server = serve().await;
    let client = client();
    let small = 512 * 1024;
    let response = client
        .send(
            &hosts("ext"),
            get_request(&server, &format!("/sized/{}", small + 1)).max_response_bytes(small),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.text().await,
        Err(Error::ResponseTooLarge { limit, .. }) if limit == small
    ));

    let over = MAX_RESPONSE_BYTES + 1;
    let response = client
        .send(
            &hosts("ext"),
            get_request(&server, &format!("/sized/{over}")).max_response_bytes(u64::MAX),
        )
        .await
        .unwrap();
    assert!(matches!(
        response.bytes().await,
        Err(Error::ResponseTooLarge { limit, .. }) if limit == MAX_RESPONSE_BYTES
    ));
}

#[tokio::test]
async fn a_request_body_over_one_mebibyte_is_refused() {
    let server = serve().await;
    let client = client();
    let over = vec![0; usize::try_from(MAX_REQUEST_BYTES).unwrap() + 1];
    let error = client
        .send(
            &hosts("ext"),
            Request::new(Method::Post, format!("{}/upload", server.base)).body(over),
        )
        .await
        .err()
        .unwrap();
    assert!(matches!(error, Error::RequestTooLarge { .. }), "{error}");
    assert_eq!(server.hits.count(), 0);

    let at_cap = vec![0; usize::try_from(MAX_REQUEST_BYTES).unwrap()];
    let response = client
        .send(
            &hosts("ext"),
            Request::new(Method::Post, format!("{}/upload", server.base)).body(at_cap),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert_eq!(
        response.text().await.unwrap(),
        MAX_REQUEST_BYTES.to_string()
    );
}

#[tokio::test]
async fn hop_by_hop_headers_are_stripped_and_custom_ones_kept() {
    let server = serve().await;
    let mut request = get_request(&server, "/echo");
    for (name, value) in [
        ("Host", "evil.test"),
        ("Connection", "x-smuggle"),
        ("Keep-Alive", "timeout=5"),
        ("Proxy-Authorization", "Basic c2VjcmV0"),
        ("Proxy-Authenticate", "Basic"),
        ("TE", "trailers"),
        ("Trailer", "x-after"),
        ("Transfer-Encoding", "chunked"),
        ("Upgrade", "websocket"),
        ("Content-Length", "999"),
        ("X-Api-Key", "kept"),
        ("Cookie", "explicit=1"),
    ] {
        request = request.header(name, value).unwrap();
    }
    let echoed = client()
        .send(&hosts("ext"), request)
        .await
        .unwrap()
        .text()
        .await
        .unwrap();

    let port = server.base.rsplit(':').next().unwrap();
    assert!(
        echoed.contains(&format!("host: api.test:{port}")),
        "{echoed}"
    );
    assert!(echoed.contains("x-api-key: kept"), "{echoed}");
    assert!(echoed.contains("cookie: explicit=1"), "{echoed}");
    assert!(echoed.contains("user-agent: Autumn/"), "{echoed}");
    for stripped in [
        "evil.test",
        "x-smuggle",
        "keep-alive",
        "proxy-authorization",
        "proxy-authenticate",
        "te:",
        "trailer",
        "transfer-encoding",
        "upgrade",
        "999",
    ] {
        assert!(!echoed.contains(stripped), "{stripped} leaked:\n{echoed}");
    }
}

#[tokio::test]
async fn a_user_agent_can_be_overridden_per_request() {
    let server = serve().await;
    let echoed = client()
        .send(
            &hosts("ext"),
            get_request(&server, "/echo")
                .header("User-Agent", "Mozilla/5.0")
                .unwrap(),
        )
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(echoed.contains("user-agent: Mozilla/5.0"), "{echoed}");
}

#[tokio::test]
async fn set_cookie_values_are_returned_and_not_replayed() {
    let server = serve().await;
    let client = client();
    let response = client
        .send(&hosts("ext"), get_request(&server, "/cookie"))
        .await
        .unwrap();
    assert_eq!(
        response.header_values("set-cookie"),
        ["session=abc; Path=/", "theme=dark; Path=/"]
    );

    let echoed = client
        .send(&hosts("ext"), get_request(&server, "/echo"))
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(!echoed.contains("cookie"), "{echoed}");
}

#[tokio::test]
async fn a_503_is_not_retried() {
    let server = serve().await;
    let response = client()
        .send(&hosts("ext"), get_request(&server, "/unavailable"))
        .await
        .unwrap();
    assert_eq!(response.status(), 503);
    assert_eq!(server.hits.count(), 1);
}

#[tokio::test]
async fn a_429_locks_out_only_that_extension() {
    let server = serve().await;
    let client = client();
    let error = client
        .send(&hosts("first"), get_request(&server, "/limited"))
        .await
        .err()
        .unwrap();
    assert!(
        matches!(error, Error::RateLimited { retry_after, .. } if retry_after.as_secs() == 120),
        "{error}"
    );

    let again = client
        .send(&hosts("first"), get_request(&server, "/echo"))
        .await
        .err()
        .unwrap();
    assert!(matches!(again, Error::RateLimited { .. }), "{again}");
    assert_eq!(server.hits.count(), 1, "the locked-out request never left");

    let other = client
        .send(&hosts("second"), get_request(&server, "/echo"))
        .await
        .unwrap();
    assert_eq!(other.status(), 200);
}

#[derive(Clone, Default)]
struct Captured(Arc<Mutex<Vec<u8>>>);

impl io::Write for Captured {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[tokio::test]
async fn the_request_record_has_no_query_string() {
    let server = serve().await;
    let captured = Captured::default();
    let writer = captured.clone();
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_ansi(false)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    client()
        .send(&hosts("ext"), get_request(&server, "/echo?apikey=hunter2"))
        .await
        .unwrap();

    let log = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    let record = log
        .lines()
        .find(|line| line.contains("extension request"))
        .expect("a request record");
    for field in ["GET", "api.test", "status=200", "elapsed_ms="] {
        assert!(record.contains(field), "{field} missing: {record}");
    }
    assert!(!log.contains("hunter2"), "query leaked:\n{log}");
    assert!(!log.contains("apikey"), "query leaked:\n{log}");
}
