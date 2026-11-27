//! The strict loopback gate and the byte-range response body.
//!
//! The exact-segment path check follows `VortX`'s `is_loopback_torrent_url`
//! (`reference/VortX/desktop/src-tauri/src/player.rs`, GPL-3.0). Original
//! copyright stays with the `VortX` authors; the license is in
//! `reference/VortX/LICENSE`.

use std::{
    io::{self, SeekFrom},
    path::Path,
    pin::Pin,
    str::FromStr,
    sync::{Arc, atomic::Ordering},
    task::{Context, Poll},
    time::Duration,
};

use axum::{
    body::{Body, Bytes},
    extract::{Request, State},
    http::{
        HeaderValue, StatusCode, Uri,
        header::{ACCEPT_RANGES, CONTENT_LENGTH, CONTENT_RANGE, CONTENT_TYPE, RANGE},
    },
    response::Response,
};
use futures::Stream;
use tokio::{
    fs::File,
    io::{AsyncRead, AsyncSeekExt, ReadBuf},
    sync::{OwnedMutexGuard, broadcast},
    time::{Sleep, sleep},
};
use torrent::InfoHash;

use crate::{
    BoxedReader, BufferingEvent, State as RelayState, Stream as LiveStream, StreamToken,
    range::{self, ByteRange},
};

/// Chunks written to the player. Large enough for video, small enough that a
/// stalled read cannot pin a huge buffer.
const CHUNK: usize = 16 * 1024;
/// A read blocked longer than this is reported as buffering (#179 Q4).
const STALL_AFTER: Duration = Duration::from_secs(2);

/// Serves `GET`/`HEAD /t/<token>/<hash>/<index>` for a live stream only.
pub(crate) async fn stream(State(state): State<Arc<RelayState>>, request: Request) -> Response {
    if request.uri().query().is_some() {
        return not_found();
    }
    let Some((token, hash, index)) = parse_route(request.uri()) else {
        return not_found();
    };
    let stream = state.streams.read().await.get(&token).cloned();
    let Some(stream) = stream else {
        return not_found();
    };
    if stream.info_hash != hash || stream.file_index != index {
        return not_found();
    }

    let header = request
        .headers()
        .get(RANGE)
        .and_then(|value| value.to_str().ok());
    let (status, start, end) = match range::resolve(header, stream.len) {
        ByteRange::Full => (StatusCode::OK, 0, stream.len.saturating_sub(1)),
        ByteRange::Part { start, end } => (StatusCode::PARTIAL_CONTENT, start, end),
        ByteRange::Unsatisfiable => return unsatisfiable(stream.len),
    };
    let count = if stream.len == 0 {
        0
    } else {
        end.saturating_sub(start) + 1
    };

    let mut response = Response::new(Body::empty());
    *response.status_mut() = status;
    let headers = response.headers_mut();
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(CONTENT_TYPE, content_type(&stream.disk_path));
    if let Ok(value) = HeaderValue::from_str(&count.to_string()) {
        headers.insert(CONTENT_LENGTH, value);
    }
    if status == StatusCode::PARTIAL_CONTENT
        && let Ok(value) = HeaderValue::from_str(&format!("bytes {start}-{end}/{}", stream.len))
    {
        headers.insert(CONTENT_RANGE, value);
    }
    if request.method() == axum::http::Method::HEAD {
        return response;
    }

    let reader = if stream.complete.load(Ordering::Acquire) {
        match File::open(&stream.disk_path).await {
            Ok(mut file) => {
                if let Err(error) = file.seek(SeekFrom::Start(start)).await {
                    tracing::warn!(%error, "could not seek the completed stream file");
                    return server_error();
                }
                Reader::Disk(file)
            }
            Err(error) => {
                tracing::warn!(%error, path = ?stream.disk_path, "could not open the completed stream file");
                return server_error();
            }
        }
    } else {
        let mut guard = Arc::clone(&stream.reader).lock_owned().await;
        if let Err(error) = guard.seek(SeekFrom::Start(start)).await {
            tracing::warn!(%error, "could not seek the torrent stream");
            return server_error();
        }
        Reader::Torrent(guard)
    };

    let body = StreamBody {
        reader,
        remaining: count,
        stall: Box::pin(sleep(STALL_AFTER)),
        stall_sent: false,
        buffering: state.buffering.clone(),
        token,
        live: Arc::clone(&stream),
        done: false,
    };
    *response.body_mut() = Body::from_stream(body);
    response
}

/// Router fallback: the same uniform 404 for unmatched paths and methods.
pub(crate) async fn not_found_handler() -> Response {
    not_found()
}

/// Uniform 404 for every request that is not an exact live-stream URL.
pub(crate) fn not_found() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::NOT_FOUND;
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
}

fn unsatisfiable(len: u64) -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::RANGE_NOT_SATISFIABLE;
    let headers = response.headers_mut();
    headers.insert(ACCEPT_RANGES, HeaderValue::from_static("bytes"));
    headers.insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    if let Ok(value) = HeaderValue::from_str(&format!("bytes */{len}")) {
        headers.insert(CONTENT_RANGE, value);
    }
    response
}

fn server_error() -> Response {
    let mut response = Response::new(Body::empty());
    *response.status_mut() = StatusCode::INTERNAL_SERVER_ERROR;
    response
        .headers_mut()
        .insert(CONTENT_LENGTH, HeaderValue::from_static("0"));
    response
}

/// Parses exactly `/t/<token>/<40-hex hash>/<u32 index>`, with no extra
/// segments, following the `VortX` strict gate (see module comment).
fn parse_route(uri: &Uri) -> Option<(StreamToken, InfoHash, u32)> {
    let mut parts = uri.path().split('/');
    let (Some(""), Some("t"), Some(token), Some(hash), Some(index), None) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return None;
    };
    let token = StreamToken::from_str(token).ok()?;
    let hash = InfoHash::from_str(hash).ok()?;
    if index.is_empty() || !index.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let index = index.parse::<u32>().ok()?;
    Some((token, hash, index))
}

/// The player gets the media type from the file it was told it was playing.
fn content_type(path: &Path) -> HeaderValue {
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    HeaderValue::from_static(match extension.as_str() {
        "mp4" | "m4v" => "video/mp4",
        "mkv" => "video/x-matroska",
        "webm" => "video/webm",
        "avi" => "video/x-msvideo",
        "mov" => "video/quicktime",
        "ts" | "m2ts" => "video/mp2t",
        "wmv" => "video/x-ms-wmv",
        "flv" => "video/x-flv",
        "mpg" | "mpeg" => "video/mpeg",
        _ => "application/octet-stream",
    })
}

/// The reader behind one response: a shared torrent stream or a disk file.
enum Reader {
    Torrent(OwnedMutexGuard<BoxedReader>),
    Disk(File),
}

impl Reader {
    fn poll_read(
        &mut self,
        cx: &mut Context<'_>,
        buffer: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        match self {
            Self::Torrent(guard) => Pin::new(&mut **guard).poll_read(cx, buffer),
            Self::Disk(file) => Pin::new(file).poll_read(cx, buffer),
        }
    }
}

/// Streams exactly the resolved byte count and cancels on disconnect or
/// revoke. A read that blocks over two seconds reports a buffering event.
struct StreamBody {
    reader: Reader,
    remaining: u64,
    stall: Pin<Box<Sleep>>,
    stall_sent: bool,
    buffering: broadcast::Sender<BufferingEvent>,
    token: StreamToken,
    live: Arc<LiveStream>,
    done: bool,
}

impl Stream for StreamBody {
    type Item = Result<Bytes, io::Error>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = &mut *self;
        if this.done {
            return Poll::Ready(None);
        }
        // Dropping this stream on client disconnect drops the reader with it.
        // Registering our waker lets a revoke interrupt a read blocked on
        // missing pieces instead of waiting for the stall timer to tick.
        this.live.waker.register(cx.waker());
        if this.live.cancelled.load(Ordering::Acquire) {
            this.done = true;
            return Poll::Ready(None);
        }
        if !this.stall_sent && this.stall.as_mut().poll(cx).is_ready() {
            this.stall_sent = true;
            let _ = this
                .buffering
                .send(BufferingEvent::Stalled { token: this.token });
        }
        if this.remaining == 0 {
            this.done = true;
            return Poll::Ready(None);
        }

        let wanted: usize = this.remaining.try_into().unwrap_or(usize::MAX).min(CHUNK);
        let mut buffer = [0u8; CHUNK];
        let Some(slice) = buffer.get_mut(..wanted) else {
            this.done = true;
            return Poll::Ready(None);
        };
        let mut read = ReadBuf::new(slice);
        match this.reader.poll_read(cx, &mut read) {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Ok(())) => {
                let filled = read.filled();
                if filled.is_empty() {
                    this.done = true;
                    return Poll::Ready(None);
                }
                if this.stall_sent {
                    let _ = this
                        .buffering
                        .send(BufferingEvent::Ready { token: this.token });
                    this.stall_sent = false;
                    this.stall = Box::pin(sleep(STALL_AFTER));
                }
                let taken = u64::try_from(filled.len()).unwrap_or(0);
                this.remaining = this.remaining.saturating_sub(taken);
                Poll::Ready(Some(Ok(Bytes::copy_from_slice(filled))))
            }
            Poll::Ready(Err(error)) => {
                this.done = true;
                Poll::Ready(Some(Err(error)))
            }
        }
    }
}
