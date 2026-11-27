//! Autumn's embedded torrent session: one librqbit session whose files live in
//! the disposable torrent cache (ADR-0006).
//!
//! A torrent is added with a single selected file, streamed through
//! [`FileStream`], and reported ready once its head and tail windows are
//! downloaded (the readiness gate, #179 Q2). Nothing persists across runs:
//! no `session.json`, no fastresume, and [`TorrentSession::start`] sweeps
//! whatever a crash left behind.

use std::{
    collections::HashSet,
    ffi::OsStr,
    fmt,
    io::{self, SeekFrom},
    net::{Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    pin::Pin,
    str::FromStr,
    sync::Arc,
    task::{Context, Poll},
    time::Duration,
};

use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, Api, DhtSessionConfig, ListenerOptions,
    Magnet, ManagedTorrent, Session, SessionOptions, api::TorrentIdOrHash, dht::Id20,
    torrent_from_bytes,
};
use tokio::io::{AsyncRead, AsyncSeek, AsyncSeekExt, ReadBuf};

mod gate;

/// How often [`TorrentSession::wait_ready`] re-checks the readiness gate.
const GATE_POLL: Duration = Duration::from_millis(100);

type BoxError = Box<dyn std::error::Error + Send + Sync>;

/// Errors from the torrent session.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// librqbit refused or failed an operation.
    #[error("torrent session failed")]
    Session(#[source] BoxError),
    /// The magnet link or `.torrent` bytes could not be parsed.
    #[error("not a valid magnet link or .torrent file")]
    InvalidSource(#[source] BoxError),
    /// No torrent with this infohash is in the session.
    #[error("torrent {0} is not in the session")]
    NotFound(InfoHash),
    /// The torrent has no file at this index, or its metadata is not known yet.
    #[error("torrent {info_hash} has no file {file_index}")]
    NoSuchFile {
        /// The torrent asked about.
        info_hash: InfoHash,
        /// The file index that was not found.
        file_index: usize,
    },
    /// librqbit takes output folders as UTF-8 strings.
    #[error("torrent cache path is not UTF-8: {}", .0.display())]
    NonUtf8Path(PathBuf),
    /// Creating or sweeping the torrent cache failed.
    #[error("torrent cache I/O failed")]
    Io(#[from] io::Error),
}

fn session_err(e: impl Into<BoxError>) -> Error {
    Error::Session(e.into())
}

/// A v1 torrent infohash (SHA-1 of the info dictionary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct InfoHash([u8; 20]);

impl InfoHash {
    fn id(self) -> TorrentIdOrHash {
        TorrentIdOrHash::Hash(Id20::new(self.0))
    }
}

impl fmt::Display for InfoHash {
    /// Lowercase hex, the form used for cache directory names.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&Id20::new(self.0).as_string())
    }
}

impl FromStr for InfoHash {
    type Err = Error;

    /// Parses 40 hex characters.
    fn from_str(s: &str) -> Result<Self, Error> {
        Id20::from_str(s)
            .map(|id| Self(id.0))
            .map_err(|e| Error::InvalidSource(e.into()))
    }
}

/// Where a torrent comes from.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum TorrentSource {
    /// A `magnet:` link carrying a v1 infohash.
    Magnet(String),
    /// The bytes of a `.torrent` file.
    File(Vec<u8>),
}

/// One file of a torrent, as listed in its metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TorrentFile {
    /// The index [`TorrentSession::add`] selects it by.
    pub index: usize,
    /// The `/`-separated path inside the torrent, without the torrent's name.
    pub path: String,
    /// Size in bytes.
    pub len: u64,
}

impl TorrentFile {
    /// A file at `index` with its path inside the torrent and its size.
    #[must_use]
    pub fn new(index: usize, path: impl Into<String>, len: u64) -> Self {
        Self {
            index,
            path: path.into(),
            len,
        }
    }
}

/// How the session finds peers.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum PeerDiscovery {
    /// DHT, trackers and the listen port on every interface.
    Public,
    /// Only these peers: no DHT, trackers or local discovery, listening on
    /// loopback. For tests against an in-process seeder.
    Only(Vec<SocketAddr>),
}

/// Progress of one readiness window, in whole pieces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowProgress {
    /// Pieces of the window that are downloaded and verified.
    pub done: u32,
    /// Pieces in the window.
    pub total: u32,
}

impl WindowProgress {
    /// Whether every piece of the window is downloaded.
    pub fn is_complete(self) -> bool {
        self.done == self.total
    }
}

/// Head and tail window progress of one streamed file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Readiness {
    /// The first 8 MiB, or the whole file when it is under 12 MiB.
    pub head: WindowProgress,
    /// The last 4 MiB; empty (`total == 0`) when the head covers the file.
    pub tail: WindowProgress,
}

impl Readiness {
    /// Whether the readiness gate is met: both windows are downloaded.
    pub fn is_ready(self) -> bool {
        self.head.is_complete() && self.tail.is_complete()
    }
}

/// A point-in-time view of one torrent, cheap enough to sample at 4 Hz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TorrentStats {
    /// Bytes of the selected files that are downloaded and verified.
    pub selected_done_bytes: u64,
    /// Total size of the selected files.
    pub selected_bytes: u64,
    /// Whether every selected file is complete.
    pub finished: bool,
    /// Whether the torrent is paused.
    pub paused: bool,
    /// Current download speed, bytes per second.
    pub download_bps: u64,
    /// Current upload speed, bytes per second.
    pub upload_bps: u64,
    /// Connected peers.
    pub peers: u32,
    /// Bytes fetched from peers this run, including discarded ones.
    pub downloaded_bytes: u64,
    /// Bytes sent to peers this run.
    pub uploaded_bytes: u64,
}

/// A readable, seekable view of one file of a torrent.
///
/// A read of a piece that is not downloaded yet waits until it arrives; a
/// seek moves this stream's 32 MiB priority window to the new position.
pub struct FileStream(Box<dyn ReadSeek>);

/// librqbit's stream type is not nameable outside the crate, so it is boxed.
trait ReadSeek: AsyncRead + AsyncSeek + Send + Unpin {}
impl<T: AsyncRead + AsyncSeek + Send + Unpin> ReadSeek for T {}

impl fmt::Debug for FileStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileStream").finish_non_exhaustive()
    }
}

impl AsyncRead for FileStream {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.0).poll_read(cx, buf)
    }
}

impl AsyncSeek for FileStream {
    fn start_seek(mut self: Pin<&mut Self>, position: SeekFrom) -> io::Result<()> {
        Pin::new(&mut self.0).start_seek(position)
    }

    fn poll_complete(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<u64>> {
        Pin::new(&mut self.0).poll_complete(cx)
    }
}

/// The embedded librqbit session.
///
/// Each torrent's files land in `<root>/<infohash>/`. Call
/// [`shutdown`](Self::shutdown) on quit to delete them.
pub struct TorrentSession {
    api: Api,
    root: PathBuf,
    discovery: PeerDiscovery,
}

impl fmt::Debug for TorrentSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TorrentSession")
            .field("root", &self.root)
            .field("discovery", &self.discovery)
            .finish_non_exhaustive()
    }
}

impl TorrentSession {
    /// Starts a session whose torrents live under `root` (the torrent cache,
    /// `<cache>/torrents`).
    ///
    /// Infohash directories left in `root` by an earlier run are deleted first:
    /// nothing resumes across launches.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if `root` can't be created or swept, [`Error::Session`]
    /// if librqbit fails to start.
    pub async fn start(root: PathBuf, discovery: PeerDiscovery) -> Result<Self, Error> {
        tokio::fs::create_dir_all(&root).await?;
        sweep(&root).await?;
        let local = matches!(discovery, PeerDiscovery::Only(_));
        let listen_addr = if local {
            (Ipv4Addr::LOCALHOST, 0).into()
        } else {
            ListenerOptions::default().listen_addr
        };
        let options = SessionOptions {
            // Without persistence, so librqbit writes nothing outside `root`.
            dht: (!local).then(|| DhtSessionConfig {
                persistence: None,
                ..DhtSessionConfig::default()
            }),
            disable_trackers: local,
            disable_local_service_discovery: local,
            fastresume: false,
            persistence: None,
            listen: Some(ListenerOptions {
                listen_addr,
                ..ListenerOptions::default()
            }),
            ..SessionOptions::default()
        };
        let session = Session::new_with_opts(root.clone(), options)
            .await
            .map_err(session_err)?;
        Ok(Self {
            api: Api::new(session, None),
            root,
            discovery,
        })
    }

    /// The address peers can connect to for this session's uploads.
    pub fn listen_addr(&self) -> Option<SocketAddr> {
        self.api.session().listen_addr()
    }

    /// Adds a torrent with only `file_index` selected, and returns its infohash.
    ///
    /// For a magnet link this waits until the metadata arrives from peers. If
    /// the torrent is already in the session it is left as it is; use
    /// [`update_only_files`](Self::update_only_files) to change its selection.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidSource`] for an unparsable source, [`Error::Session`]
    /// if librqbit rejects it (for example, an out-of-range `file_index`).
    pub async fn add(&self, source: TorrentSource, file_index: usize) -> Result<InfoHash, Error> {
        let (info_hash, add) = match source {
            TorrentSource::Magnet(url) => {
                let magnet = Magnet::parse(&url).map_err(|e| Error::InvalidSource(e.into()))?;
                let id = magnet
                    .as_id20()
                    .ok_or_else(|| Error::InvalidSource("magnet link has no v1 infohash".into()))?;
                (InfoHash(id.0), AddTorrent::from_url(url))
            }
            TorrentSource::File(bytes) => {
                let meta =
                    torrent_from_bytes(&bytes).map_err(|e| Error::InvalidSource(e.into()))?;
                (InfoHash(meta.info_hash.0), AddTorrent::from_bytes(bytes))
            }
        };
        let folder = self.dir(info_hash);
        let output_folder = folder
            .to_str()
            .ok_or_else(|| Error::NonUtf8Path(folder.clone()))?
            .to_owned();
        let initial_peers = match &self.discovery {
            PeerDiscovery::Public => None,
            PeerDiscovery::Only(peers) => Some(peers.clone()),
        };
        let options = AddTorrentOptions {
            only_files: Some(vec![file_index]),
            output_folder: Some(output_folder),
            initial_peers,
            ..AddTorrentOptions::default()
        };
        self.api
            .session()
            .add_torrent(add, Some(options))
            .await
            .map_err(session_err)?;
        Ok(info_hash)
    }

    /// Lists a torrent's files without adding it or downloading anything.
    ///
    /// A `.torrent` is read directly; a magnet link waits until its metadata
    /// arrives from peers, so callers should bound the wait. Padding files
    /// are left out, and each file keeps the index [`add`](Self::add) takes.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidSource`] for an unparsable source, [`Error::Session`]
    /// if librqbit cannot resolve the metadata.
    pub async fn list_files(&self, source: TorrentSource) -> Result<Vec<TorrentFile>, Error> {
        let add = match source {
            TorrentSource::Magnet(url) => {
                Magnet::parse(&url).map_err(|e| Error::InvalidSource(e.into()))?;
                AddTorrent::from_url(url)
            }
            TorrentSource::File(bytes) => {
                torrent_from_bytes(&bytes).map_err(|e| Error::InvalidSource(e.into()))?;
                AddTorrent::from_bytes(bytes)
            }
        };
        let initial_peers = match &self.discovery {
            PeerDiscovery::Public => None,
            PeerDiscovery::Only(peers) => Some(peers.clone()),
        };
        let options = AddTorrentOptions {
            list_only: true,
            initial_peers,
            ..AddTorrentOptions::default()
        };
        let response = self
            .api
            .session()
            .add_torrent(add, Some(options))
            .await
            .map_err(session_err)?;
        let files = match response {
            AddTorrentResponse::ListOnly(listed) => listed
                .info
                .iter_file_details()
                .enumerate()
                .filter(|(_, file)| !file.attrs().padding)
                .map(|(index, file)| TorrentFile {
                    index,
                    path: file.filename.to_vec().join("/"),
                    len: file.len,
                })
                .collect(),
            AddTorrentResponse::AlreadyManaged(_, handle)
            | AddTorrentResponse::Added(_, handle) => handle
                .with_metadata(|meta| {
                    meta.file_infos
                        .iter()
                        .enumerate()
                        .filter(|(_, file)| !file.attrs.padding)
                        .map(|(index, file)| TorrentFile {
                            index,
                            path: file
                                .relative_filename
                                .components()
                                .map(|part| part.as_os_str().to_string_lossy())
                                .collect::<Vec<_>>()
                                .join("/"),
                            len: file.len,
                        })
                        .collect()
                })
                .map_err(session_err)?,
        };
        Ok(files)
    }

    /// Opens a stream over one file of a torrent, waiting for the torrent to
    /// finish checking existing data first.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if the torrent is not in the session,
    /// [`Error::Session`] if librqbit can't open the file.
    pub async fn stream(
        &self,
        info_hash: InfoHash,
        file_index: usize,
    ) -> Result<FileStream, Error> {
        let handle = self.handle(info_hash)?;
        let stream = handle.stream(file_index).await.map_err(session_err)?;
        Ok(FileStream(Box::new(stream)))
    }

    /// Head and tail window progress of one file.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], [`Error::NoSuchFile`] (also while a magnet's
    /// metadata is unknown), or [`Error::Session`] while the torrent is still
    /// initializing.
    pub fn readiness(&self, info_hash: InfoHash, file_index: usize) -> Result<Readiness, Error> {
        let handle = self.handle(info_hash)?;
        let no_file = || Error::NoSuchFile {
            info_hash,
            file_index,
        };
        let windows = handle
            .with_metadata(|meta| {
                let file = meta.file_infos.get(file_index)?;
                gate::windows(
                    file.offset_in_torrent,
                    file.len,
                    meta.lengths().default_piece_length(),
                )
            })
            .map_err(|_| no_file())?
            .ok_or_else(no_file)?;
        let (haves, _) = self
            .api
            .api_dump_haves(info_hash.id())
            .map_err(session_err)?;
        let progress = |pieces: std::ops::Range<u32>| {
            let total = pieces.end - pieces.start;
            let done = pieces
                .filter(|&i| {
                    usize::try_from(i)
                        .ok()
                        .and_then(|i| haves.get(i))
                        .is_some_and(|bit| *bit)
                })
                .count();
            WindowProgress {
                done: u32::try_from(done).unwrap_or(total),
                total,
            }
        };
        Ok(Readiness {
            head: progress(windows.head),
            tail: progress(windows.tail),
        })
    }

    /// Waits until the readiness gate of one file is met.
    ///
    /// A head stream and a second stream positioned at the tail window are held
    /// open while waiting, so librqbit fetches both ends first. Dropping the
    /// future cancels the wait.
    ///
    /// # Errors
    ///
    /// As [`stream`](Self::stream) and [`readiness`](Self::readiness).
    pub async fn wait_ready(&self, info_hash: InfoHash, file_index: usize) -> Result<(), Error> {
        let _head = self.stream(info_hash, file_index).await?;
        let mut tail = self.stream(info_hash, file_index).await?;
        let file_len = tail.seek(SeekFrom::End(0)).await?;
        if let Some(start) = gate::tail_start(file_len) {
            tail.seek(SeekFrom::Start(start)).await?;
        }
        while !self.readiness(info_hash, file_index)?.is_ready() {
            tokio::time::sleep(GATE_POLL).await;
        }
        Ok(())
    }

    /// Stops downloading and uploading a torrent.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], or [`Error::Session`] if it is already paused.
    pub async fn pause(&self, info_hash: InfoHash) -> Result<(), Error> {
        let handle = self.handle(info_hash)?;
        self.api.session().pause(&handle).await.map_err(session_err)
    }

    /// Resumes a paused torrent.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], or [`Error::Session`] if it is not paused.
    pub async fn resume(&self, info_hash: InfoHash) -> Result<(), Error> {
        let handle = self.handle(info_hash)?;
        self.api
            .session()
            .unpause(&handle)
            .await
            .map_err(session_err)
    }

    /// Replaces the set of selected files, for example to add the next
    /// episode of a batch.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`], or [`Error::Session`] for an out-of-range index.
    pub async fn update_only_files(
        &self,
        info_hash: InfoHash,
        file_indices: &[usize],
    ) -> Result<(), Error> {
        let handle = self.handle(info_hash)?;
        let files: HashSet<usize> = file_indices.iter().copied().collect();
        self.api
            .session()
            .update_only_files(&handle, &files)
            .await
            .map_err(session_err)
    }

    /// Removes a torrent from the session and deletes its directory.
    ///
    /// # Errors
    ///
    /// [`Error::Session`] if librqbit fails, [`Error::Io`] if the directory
    /// can't be removed.
    pub async fn delete(&self, info_hash: InfoHash) -> Result<(), Error> {
        self.api
            .session()
            .delete(info_hash.id(), true)
            .await
            .map_err(session_err)?;
        remove_dir_if_present(&self.dir(info_hash)).await
    }

    /// Ends a torrent stream: the stream drop (ADR-0006).
    ///
    /// A torrent whose selected files are incomplete is deleted with its
    /// files. A complete one keeps seeding (and is resumed if paused) until
    /// [`shutdown`](Self::shutdown).
    ///
    /// # Errors
    ///
    /// As [`delete`](Self::delete) and [`resume`](Self::resume).
    pub async fn drop_stream(&self, info_hash: InfoHash) -> Result<(), Error> {
        let handle = self.handle(info_hash)?;
        if !handle.stats().finished {
            return self.delete(info_hash).await;
        }
        if handle.is_paused() {
            self.resume(info_hash).await?;
        }
        Ok(())
    }

    /// A snapshot of one torrent's progress and transfer stats.
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if the torrent is not in the session.
    pub fn stats(&self, info_hash: InfoHash) -> Result<TorrentStats, Error> {
        let handle = self.handle(info_hash)?;
        let stats = handle.stats();
        let mut out = TorrentStats {
            selected_done_bytes: stats.progress_bytes,
            selected_bytes: stats.total_bytes,
            finished: stats.finished,
            paused: handle.is_paused(),
            download_bps: 0,
            upload_bps: 0,
            peers: 0,
            downloaded_bytes: 0,
            uploaded_bytes: stats.uploaded_bytes,
        };
        if let Some(live) = handle.live() {
            let snapshot = live.stats_snapshot();
            out.download_bps = live.down_speed_estimator().bps();
            out.upload_bps = live.up_speed_estimator().bps();
            out.peers = snapshot.peer_stats.live;
            out.downloaded_bytes = snapshot.fetched_bytes;
            out.uploaded_bytes = snapshot.uploaded_bytes;
        }
        Ok(out)
    }

    /// Quit: drops every torrent, including seeding ones, stops the session
    /// and deletes every infohash directory under the cache root.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if a directory can't be removed. Torrents librqbit fails
    /// to delete are still swept from disk.
    pub async fn shutdown(self) -> Result<(), Error> {
        let session = self.api.session();
        let ids = session.with_torrents(|torrents| torrents.map(|(id, _)| id).collect::<Vec<_>>());
        for id in ids {
            if let Err(error) = session.delete(TorrentIdOrHash::Id(id), true).await {
                tracing::warn!(%error, "could not delete torrent on quit");
            }
        }
        session.stop().await;
        sweep(&self.root).await
    }

    fn dir(&self, info_hash: InfoHash) -> PathBuf {
        self.root.join(info_hash.to_string())
    }

    fn handle(&self, info_hash: InfoHash) -> Result<Arc<ManagedTorrent>, Error> {
        self.api
            .session()
            .get(info_hash.id())
            .ok_or(Error::NotFound(info_hash))
    }
}

/// The disposable torrent cache on disk, independent of a running session.
///
/// `clear` accepts the infohashes of live streams and leaves their files
/// alone. A caller with a session passes its active infohashes; without one,
/// an empty slice clears everything (ARCHITECTURE §6, ADR-0006).
#[derive(Debug, Clone)]
pub struct TorrentCache {
    root: PathBuf,
}

impl TorrentCache {
    /// Views the cache rooted at the session's output folder.
    #[must_use]
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The total size in bytes of the files in infohash directories.
    ///
    /// A cache that has not been created yet has size zero.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if a directory cannot be read.
    pub async fn size_bytes(&self) -> Result<u64, Error> {
        let dirs = match infohash_dirs(&self.root).await {
            Ok(dirs) => dirs,
            Err(Error::Io(source)) if source.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        let mut total = 0u64;
        for dir in dirs {
            total = total.saturating_add(dir_size(&dir).await?);
        }
        Ok(total)
    }

    /// Deletes every cached torrent except the ones still streaming, then
    /// reports the remaining size.
    ///
    /// Only infohash-named directories are removed, so a user-chosen folder
    /// keeps anything else it contains. A live infohash and its files are
    /// left untouched; the caller owns the session and knows which those are.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if a directory cannot be read or removed.
    pub async fn clear(&self, active: &[InfoHash]) -> Result<u64, Error> {
        let active: HashSet<InfoHash> = active.iter().copied().collect();
        let dirs = match infohash_dirs(&self.root).await {
            Ok(dirs) => dirs,
            Err(Error::Io(source)) if source.kind() == io::ErrorKind::NotFound => return Ok(0),
            Err(error) => return Err(error),
        };
        for dir in dirs {
            let Some(name) = dir.file_name() else {
                continue;
            };
            if let Some(hash) = parse_infohash_dir(name)
                && !active.contains(&hash)
            {
                remove_dir_if_present(&dir).await?;
            }
        }
        self.size_bytes().await
    }
}

/// Lists the infohash directories directly under `root`.
async fn infohash_dirs(root: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut entries = tokio::fs::read_dir(root).await?;
    let mut dirs = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        if parse_infohash_dir(&entry.file_name()).is_some() && entry.file_type().await?.is_dir() {
            dirs.push(entry.path());
        }
    }
    Ok(dirs)
}

/// Sums file sizes under `dir`, walking directories but not symlinks.
async fn dir_size(dir: &Path) -> Result<u64, Error> {
    let mut pending = vec![dir.to_path_buf()];
    let mut total = 0u64;
    while let Some(dir) = pending.pop() {
        let mut entries = tokio::fs::read_dir(&dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            let kind = entry.file_type().await?;
            if kind.is_dir() {
                pending.push(entry.path());
            } else if kind.is_file() {
                total = total.saturating_add(entry.metadata().await?.len());
            }
        }
    }
    Ok(total)
}

/// Recognizes the `<infohash>` directory names librqbit writes under a cache.
fn parse_infohash_dir(name: &OsStr) -> Option<InfoHash> {
    let name = name.to_str()?;
    if name.len() != 40 || !name.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    InfoHash::from_str(&name.to_ascii_lowercase()).ok()
}

/// Deletes every infohash-named directory directly under `root`. Anything
/// else is left alone, in case `root` is ever a user-chosen folder.
async fn sweep(root: &Path) -> Result<(), Error> {
    let mut entries = tokio::fs::read_dir(root).await?;
    while let Some(entry) = entries.next_entry().await? {
        let is_infohash = parse_infohash_dir(&entry.file_name()).is_some();
        if is_infohash && entry.file_type().await?.is_dir() {
            remove_dir_if_present(&entry.path()).await?;
        }
    }
    Ok(())
}

async fn remove_dir_if_present(dir: &Path) -> Result<(), Error> {
    match tokio::fs::remove_dir_all(dir).await {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e.into()),
        _ => Ok(()),
    }
}
