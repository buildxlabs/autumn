//! Loopback relay for torrent streams (ADR-0005, #179 Q3–Q5, ROADMAP Phase 3).
//!
//! The player is always handed a URL on this server, never a torrent file
//! path, even after the file completes. One 128-bit token gates each stream;
//! the engine mints it when it resolves a source and revokes it when the
//! playback session ends. Tokens are redacted in logs and in `Debug` output.
//!
//! The engine supplies a `FileStream` at [`Relay::mint`] and the path the file
//! will occupy on disk. Once the torrent is complete it calls
//! [`Relay::mark_complete`], after which the relay reads that file from disk
//! at the same URL.

use std::{
    collections::HashMap,
    fmt, io,
    net::{Ipv4Addr, SocketAddr},
    path::PathBuf,
    str::FromStr,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

use axum::{
    Router,
    routing::{MethodFilter, on},
};
use domain::InfoHash;
use futures::task::AtomicWaker;
use rand::random;
use tokio::{
    io::{AsyncRead, AsyncSeek},
    net::TcpListener,
    sync::{RwLock, broadcast, oneshot},
    task::JoinHandle,
};

mod range;
mod serve;

/// Errors from starting the relay or parsing a URL token.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The loopback listener could not be bound.
    #[error("could not bind the loopback relay")]
    Bind(#[source] io::Error),
    /// A URL segment was not a 128-bit token in lowercase or uppercase hex.
    #[error("malformed stream token")]
    MalformedToken,
}

/// The 128-bit secret in one stream's URL.
///
/// `Debug` never prints the secret so a log cannot hand a live URL to a
/// local reader.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct StreamToken([u8; 16]);

impl StreamToken {
    fn mint() -> Self {
        Self(random::<u128>().to_be_bytes())
    }

    fn hex(self) -> String {
        format!("{:032x}", u128::from_be_bytes(self.0))
    }
}

impl fmt::Debug for StreamToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StreamToken(<redacted>)")
    }
}

impl FromStr for StreamToken {
    type Err = Error;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.len() != 32 || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(Error::MalformedToken);
        }
        let value = u128::from_str_radix(text, 16).map_err(|_| Error::MalformedToken)?;
        Ok(Self(value.to_be_bytes()))
    }
}

/// A change in one stream's buffering state, for the engine's `Buffering` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferingEvent {
    /// A read waited on missing pieces for more than two seconds.
    Stalled {
        /// The stream whose read is blocked.
        token: StreamToken,
    },
    /// Bytes arrived after a stall.
    Ready {
        /// The stream that is flowing again.
        token: StreamToken,
    },
}

/// What the engine hands the relay when it resolves a torrent source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamSpec {
    /// Identifies the torrent whose file is streamed.
    pub info_hash: InfoHash,
    /// The selected file's index in the torrent.
    pub file_index: usize,
    /// The file's length in bytes, for `Content-Range` and `Content-Length`.
    pub len: u64,
    /// Where the file lands in the torrent cache once it is complete.
    pub disk_path: PathBuf,
}

/// The reader a live stream holds: a torrent `FileStream` in practice.
pub(crate) trait ReadSeek: AsyncRead + AsyncSeek + Send + Unpin {}
impl<T: AsyncRead + AsyncSeek + Send + Unpin> ReadSeek for T {}

/// A boxed reader, so the relay can hold a `FileStream` without naming it.
pub(crate) type BoxedReader = Box<dyn ReadSeek>;

/// One live stream in the gate's registry.
pub(crate) struct Stream {
    pub(crate) info_hash: InfoHash,
    pub(crate) file_index: usize,
    pub(crate) len: u64,
    pub(crate) disk_path: PathBuf,
    pub(crate) complete: AtomicBool,
    pub(crate) reader: Arc<tokio::sync::Mutex<BoxedReader>>,
    pub(crate) cancelled: AtomicBool,
    pub(crate) waker: AtomicWaker,
}

impl fmt::Debug for Stream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Stream")
            .field("info_hash", &self.info_hash)
            .field("file_index", &self.file_index)
            .field("len", &self.len)
            .field("complete", &self.complete.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

/// State shared by the server task and the engine-facing API.
pub(crate) struct State {
    pub(crate) streams: RwLock<HashMap<StreamToken, Arc<Stream>>>,
    pub(crate) buffering: broadcast::Sender<BufferingEvent>,
}

/// The running loopback relay. Dropping it stops the server.
pub struct Relay {
    addr: SocketAddr,
    state: Arc<State>,
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl fmt::Debug for Relay {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Relay")
            .field("addr", &self.addr)
            .finish_non_exhaustive()
    }
}

impl Relay {
    /// Binds an OS-chosen port on `127.0.0.1` and starts serving in the
    /// background. Call this lazily, on the first torrent stream.
    ///
    /// # Errors
    ///
    /// [`Error::Bind`] if the loopback listener cannot be bound.
    pub async fn start() -> Result<Self, Error> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
            .await
            .map_err(Error::Bind)?;
        let addr = listener.local_addr().map_err(Error::Bind)?;
        let (buffering, _) = broadcast::channel(64);
        let state = Arc::new(State {
            streams: RwLock::new(HashMap::new()),
            buffering,
        });
        let router = Router::new()
            .route(
                "/t/{token}/{hash}/{index}",
                on(MethodFilter::GET.or(MethodFilter::HEAD), serve::stream),
            )
            .method_not_allowed_fallback(serve::not_found_handler)
            .fallback(serve::not_found_handler)
            .with_state(Arc::clone(&state));
        let (shutdown, stopped) = oneshot::channel::<()>();
        let task = tokio::spawn(async move {
            if let Err(error) = axum::serve(listener, router)
                .with_graceful_shutdown(async {
                    let _ = stopped.await;
                })
                .await
            {
                tracing::error!(%error, "relay server stopped with an error");
            }
        });
        Ok(Self {
            addr,
            state,
            shutdown: Some(shutdown),
            task,
        })
    }

    /// The bound `127.0.0.1:<port>` address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// Registers a live stream and returns its token and player URL.
    ///
    /// `reader` is the torrent's `FileStream`; reads of missing pieces block
    /// on it with no relay-side timeout. The engine revokes the token when
    /// the session ends.
    pub async fn mint<R>(&self, spec: StreamSpec, reader: R) -> (StreamToken, String)
    where
        R: AsyncRead + AsyncSeek + Send + Unpin + 'static,
    {
        let token = StreamToken::mint();
        let url = format!(
            "http://{}/t/{}/{}/{}",
            self.addr,
            token.hex(),
            spec.info_hash,
            spec.file_index
        );
        let boxed: BoxedReader = Box::new(reader);
        let stream = Arc::new(Stream {
            info_hash: spec.info_hash,
            file_index: spec.file_index,
            len: spec.len,
            disk_path: spec.disk_path,
            complete: AtomicBool::new(false),
            reader: Arc::new(tokio::sync::Mutex::new(boxed)),
            cancelled: AtomicBool::new(false),
            waker: AtomicWaker::new(),
        });
        self.state.streams.write().await.insert(token, stream);
        (token, url)
    }

    /// Ends a stream: a blocked read stops and its URL returns 404 again.
    /// Idempotent.
    pub async fn revoke(&self, token: &StreamToken) {
        if let Some(stream) = self.state.streams.write().await.remove(token) {
            // Waking the registered reader lets a read blocked on missing
            // pieces end now instead of at the next stall timer tick.
            stream.cancelled.store(true, Ordering::Release);
            stream.waker.wake();
        }
    }

    /// Switches a live stream to reads from its completed file on disk.
    /// Returns `false` for a token that is no longer live.
    pub async fn mark_complete(&self, token: &StreamToken) -> bool {
        let stream = self.state.streams.read().await.get(token).cloned();
        match stream {
            Some(stream) => {
                stream.complete.store(true, Ordering::Release);
                true
            }
            None => false,
        }
    }

    /// Subscribes to buffering transitions for the engine's HUD overlay.
    pub fn subscribe_buffering(&self) -> broadcast::Receiver<BufferingEvent> {
        self.state.buffering.subscribe()
    }

    /// Stops serving and waits for the server task to finish.
    pub async fn shutdown(mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
        let _ = (&mut self.task).await;
    }
}

impl Drop for Relay {
    fn drop(&mut self) {
        if let Some(shutdown) = self.shutdown.take() {
            let _ = shutdown.send(());
        }
    }
}
