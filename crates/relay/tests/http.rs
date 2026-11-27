//! HTTP behavior of the loopback relay (#216 acceptance criteria).

use std::{
    io::{self, Cursor, SeekFrom},
    net::SocketAddr,
    path::{Path, PathBuf},
    pin::Pin,
    task::{Context, Poll},
    time::Duration,
};

use reqwest::{
    Client, StatusCode,
    header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
};
use tempfile::TempDir;
use tokio::io::{AsyncRead, AsyncSeek, ReadBuf};
use torrent::InfoHash;

use relay::{BufferingEvent, Relay, StreamSpec};

const HASH: &str = "0123456789abcdef0123456789abcdef01234567";
const VIDEO: &[u8] = b"0123456789";

#[expect(
    clippy::expect_used,
    reason = "test helper: HASH is a fixed valid infohash"
)]
fn hash() -> InfoHash {
    HASH.parse::<InfoHash>().expect("valid infohash")
}

fn spec(dir: &Path, len: u64) -> StreamSpec {
    StreamSpec {
        info_hash: hash(),
        file_index: 0,
        len,
        disk_path: dir.join("episode.mkv"),
    }
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failed request fails the test"
)]
async fn get(client: &Client, url: &str, range: Option<&str>) -> reqwest::Response {
    let mut request = client.get(url);
    if let Some(range) = range {
        request = request.header(RANGE, range);
    }
    request.send().await.expect("GET the relay")
}

#[expect(
    clippy::expect_used,
    reason = "test helper: a failed body read fails the test"
)]
async fn body(response: reqwest::Response) -> Vec<u8> {
    response.bytes().await.expect("read the body").to_vec()
}

fn video(range: std::ops::Range<usize>) -> &'static [u8] {
    VIDEO.get(range).unwrap_or_default()
}

fn segment(url: &str, index: usize) -> &str {
    url.split('/').nth(index).unwrap_or_default()
}

/// A reader that never yields a byte, standing in for a missing piece.
struct Blocking;

impl AsyncRead for Blocking {
    fn poll_read(
        self: Pin<&mut Self>,
        _cx: &mut Context<'_>,
        _buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Poll::Pending
    }
}

impl AsyncSeek for Blocking {
    fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> io::Result<()> {
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(0))
    }
}

/// A reader that yields one byte every 500 ms, like a slow but flowing swarm.
struct Trickle {
    left: usize,
    delay: Pin<Box<tokio::time::Sleep>>,
}

impl Trickle {
    fn new(len: usize) -> Self {
        Self {
            left: len,
            delay: Box::pin(tokio::time::sleep(Duration::from_millis(500))),
        }
    }
}

impl AsyncRead for Trickle {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.left == 0 {
            return Poll::Ready(Ok(()));
        }
        if self.delay.as_mut().poll(cx).is_pending() {
            return Poll::Pending;
        }
        self.left -= 1;
        self.delay = Box::pin(tokio::time::sleep(Duration::from_millis(500)));
        buf.put_slice(b"x");
        Poll::Ready(Ok(()))
    }
}

impl AsyncSeek for Trickle {
    fn start_seek(self: Pin<&mut Self>, _position: SeekFrom) -> io::Result<()> {
        Ok(())
    }

    fn poll_complete(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Poll::Ready(Ok(0))
    }
}

#[tokio::test]
async fn relay_binds_an_ephemeral_loopback_port() {
    let relay = Relay::start().await.expect("start the relay");
    let addr: SocketAddr = relay.addr();
    assert!(addr.ip().is_loopback(), "relay must bind loopback only");
    assert_ne!(addr.port(), 0, "the OS must choose a port");
}

#[tokio::test]
async fn no_range_returns_the_full_entity() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;

    let response = get(&Client::new(), &url, None).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()[ACCEPT_RANGES], "bytes");
    assert_eq!(response.headers()[CONTENT_TYPE], "video/x-matroska");
    assert_eq!(response.headers()[CONTENT_LENGTH], "10");
    assert_eq!(body(response).await, VIDEO);
}

#[tokio::test]
async fn single_ranges_return_206_with_content_range() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;
    let client = Client::new();

    for (header, content_range, expected) in [
        ("bytes=2-5", "bytes 2-5/10", video(2..6)),
        ("bytes=7-", "bytes 7-9/10", video(7..10)),
        ("bytes=-3", "bytes 7-9/10", video(7..10)),
    ] {
        let response = get(&client, &url, Some(header)).await;
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT, "{header}");
        assert_eq!(response.headers()[CONTENT_RANGE], content_range, "{header}");
        assert_eq!(response.headers()[ACCEPT_RANGES], "bytes", "{header}");
        assert_eq!(body(response).await, expected, "{header}");
    }
}

#[tokio::test]
async fn a_multi_range_uses_its_first_range() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;

    let response = get(&Client::new(), &url, Some("bytes=0-1,4-5")).await;
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[CONTENT_RANGE], "bytes 0-1/10");
    assert_eq!(body(response).await, video(0..2));
}

#[tokio::test]
async fn an_out_of_file_range_returns_416() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;

    let response = get(&Client::new(), &url, Some("bytes=20-")).await;
    assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(response.headers()[CONTENT_RANGE], "bytes */10");
    assert_eq!(response.headers()[ACCEPT_RANGES], "bytes");
    assert_eq!(body(response).await, Vec::<u8>::new());
}

#[tokio::test]
async fn head_keeps_the_headers_but_drops_the_body() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;

    let response = Client::new()
        .head(&url)
        .header(RANGE, "bytes=2-5")
        .send()
        .await
        .expect("HEAD the relay");
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(response.headers()[CONTENT_RANGE], "bytes 2-5/10");
    assert_eq!(response.headers()[CONTENT_LENGTH], "4");
    assert_eq!(body(response).await, Vec::<u8>::new());
}

#[tokio::test]
async fn only_the_exact_live_stream_url_is_served() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;
    let base = format!("http://{}", relay.addr());
    let client = Client::new();

    let token = segment(&url, 4);
    let wrong_token = "ffffffffffffffffffffffffffffffff";
    let wrong_hash = "ffffffffffffffffffffffffffffffffffffffff";
    let rejected = [
        format!("{base}/t/{wrong_token}/{HASH}/0"),
        format!("{base}/t/{token}/{HASH}/1"),
        format!("{base}/t/{token}/{wrong_hash}/0"),
        format!("{url}?token=1"),
        format!("{base}/"),
        format!("{base}/t"),
        format!("{url}/extra"),
    ];

    for bad in &rejected {
        let response = get(&client, bad, None).await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND, "GET {bad}");
        assert_eq!(
            body(response).await,
            Vec::<u8>::new(),
            "404 must have no body"
        );
    }

    let response = client.post(&url).send().await.expect("POST the relay");
    assert_eq!(response.status(), StatusCode::NOT_FOUND, "POST {url}");
    assert_eq!(body(response).await, Vec::<u8>::new());
}

#[tokio::test]
async fn revoking_a_token_ends_a_blocked_read_and_closes_the_gate() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (token, url) = relay.mint(spec(dir.path(), 10), Blocking).await;
    let client = Client::new();
    let request_url = url.clone();
    let pending = tokio::spawn(async move {
        let response = get(&client, &request_url, None).await;
        response.bytes().await.map(|bytes| bytes.to_vec())
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    relay.revoke(&token).await;
    let outcome = tokio::time::timeout(Duration::from_millis(800), pending)
        .await
        .expect("revoke cancels a blocked read promptly")
        .expect("the request task joins");
    match outcome {
        Ok(bytes) => assert_eq!(bytes, Vec::<u8>::new(), "a cancelled read has no body"),
        Err(error) => assert!(error.is_decode(), "revoke cuts the body short: {error}"),
    }

    let response = get(&Client::new(), &url, None).await;
    assert_eq!(
        response.status(),
        StatusCode::NOT_FOUND,
        "revoked URL is gated"
    );
}

#[tokio::test]
async fn a_blocked_read_reports_buffering_after_two_seconds() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (token, url) = relay.mint(spec(dir.path(), 10), Blocking).await;
    let mut events = relay.subscribe_buffering();
    let client = Client::new();
    let request_url = url.clone();
    let pending = tokio::spawn(async move { get(&client, &request_url, None).await });

    let event = tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("a stall is reported")
        .expect("the buffering channel stays open");
    assert_eq!(event, BufferingEvent::Stalled { token });

    relay.revoke(&token).await;
    let _ = pending.await;
}

#[tokio::test]
async fn a_slow_but_flowing_read_never_reports_buffering() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (_token, url) = relay.mint(spec(dir.path(), 6), Trickle::new(6)).await;
    let mut events = relay.subscribe_buffering();
    let client = Client::new();

    let received = body(get(&client, &url, None).await).await;

    assert_eq!(received, b"xxxxxx", "three seconds of trickled bytes");
    assert!(
        events.try_recv().is_err(),
        "no gap reached two seconds, so no buffering event"
    );
}

#[tokio::test]
async fn a_complete_file_is_served_from_disk_at_the_same_url() {
    let dir = TempDir::new().expect("temp dir");
    let disk: PathBuf = dir.path().join("episode.mkv");
    let relay = Relay::start().await.expect("start the relay");
    let (token, url) = relay
        .mint(
            spec(dir.path(), VIDEO.len().try_into().unwrap_or(0)),
            Cursor::new(VIDEO),
        )
        .await;
    let from_disk = b"DISK-DISK-DI";
    tokio::fs::write(&disk, from_disk)
        .await
        .expect("write the disk file");
    let client = Client::new();

    let response = get(&client, &url, Some("bytes=0-3")).await;
    assert_eq!(body(response).await, video(0..4), "before completion");

    assert!(relay.mark_complete(&token).await, "the stream is live");
    let response = get(&client, &url, Some("bytes=0-3")).await;
    assert_eq!(response.headers()[CONTENT_RANGE], "bytes 0-3/10");
    assert_eq!(
        body(response).await,
        from_disk.get(..4).unwrap_or_default(),
        "after completion"
    );
}

#[tokio::test]
async fn the_token_is_redacted_in_debug_output() {
    let dir = TempDir::new().expect("temp dir");
    let relay = Relay::start().await.expect("start the relay");
    let (token, url) = relay.mint(spec(dir.path(), 10), Cursor::new(VIDEO)).await;
    let hex = segment(&url, 4);
    assert!(!format!("{token:?}").contains(hex), "Debug hides the token");
}
