//! Two in-process sessions, a librqbit seeder and an Autumn leecher, talking
//! over loopback: no internet.

#![expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test harness: setup helpers panic on failure and slices compare seeded bytes"
)]

use std::{
    io::SeekFrom,
    net::{Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    path::Path,
    sync::Arc,
    time::Duration,
};

use domain::InfoHash;
use librqbit::{
    CreateTorrentOptions, ListenerOptions, Session, SessionOptions, limits::LimitsConfig,
};
use tempfile::TempDir;
use tokio::{
    io::{AsyncReadExt, AsyncSeekExt},
    time::{sleep, timeout},
};
use torrent::{
    Error, PeerDiscovery, TorrentSession, TorrentSessionOptions, TorrentSource, TorrentStats,
};

const MIB: u64 = 1024 * 1024;
/// Upload limits are `u32` bytes per second.
const MIB_PER_S: u32 = 1024 * 1024;
const PIECE: u32 = 256 * 1024;
const LIMIT: Duration = Duration::from_secs(60);
/// Big enough that the middle of the file is far from both windows.
const EPISODE: u64 = 96 * MIB;

/// Deterministic pseudo-random bytes, so every piece hashes differently.
fn contents(len: u64) -> Vec<u8> {
    let len = usize::try_from(len).unwrap();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut buf = Vec::with_capacity(len + 8);
    while buf.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        buf.extend_from_slice(&x.to_le_bytes());
    }
    buf.truncate(len);
    buf
}

struct Seeder {
    _session: Arc<Session>,
    _dir: TempDir,
    addr: SocketAddr,
    torrent: Vec<u8>,
    content: Vec<u8>,
}

/// A plain librqbit session seeding one file, `ep.mkv`, at `upload_bps`.
async fn seeder(len: u64, upload_bps: u32) -> Seeder {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ep.mkv");
    let content = contents(len);
    std::fs::write(&path, &content).unwrap();
    let session = Session::new_with_opts(
        dir.path().into(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            disable_trackers: true,
            listen: Some(ListenerOptions {
                listen_addr: (Ipv4Addr::LOCALHOST, 0).into(),
                ..Default::default()
            }),
            ratelimits: LimitsConfig {
                upload_bps: NonZeroU32::new(upload_bps),
                download_bps: None,
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (created, handle) = session
        .create_and_serve_torrent(
            &path,
            CreateTorrentOptions {
                piece_length: Some(PIECE),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    handle.wait_until_completed().await.unwrap();
    Seeder {
        addr: session.listen_addr().unwrap(),
        torrent: created.as_bytes().unwrap().to_vec(),
        _session: session,
        _dir: dir,
        content,
    }
}

/// An Autumn session that only knows `peer`, with the seeder's torrent added.
async fn leecher(root: &Path, peer: SocketAddr, torrent: &[u8]) -> (TorrentSession, InfoHash) {
    let session = TorrentSession::start(
        root.to_path_buf(),
        PeerDiscovery::Only(vec![peer]),
        TorrentSessionOptions::default(),
    )
    .await
    .unwrap();
    let hash = session
        .add(TorrentSource::File(torrent.to_vec()), 0)
        .await
        .unwrap();
    (session, hash)
}

async fn wait_for(session: &TorrentSession, hash: InfoHash, done: impl Fn(&TorrentStats) -> bool) {
    timeout(LIMIT, async {
        while !done(&session.stats(hash).unwrap()) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn readiness_gate_fetches_head_and_tail_before_the_middle() {
    let seeder = seeder(EPISODE, 16 * MIB_PER_S).await;
    let cache = tempfile::tempdir().unwrap();
    let (session, hash) = leecher(cache.path(), seeder.addr, &seeder.torrent).await;

    timeout(LIMIT, session.wait_ready(hash, 0))
        .await
        .unwrap()
        .unwrap();

    let readiness = session.readiness(hash, 0).unwrap();
    assert!(readiness.is_ready(), "gate met: {readiness:?}");
    assert_eq!(readiness.head.total, 32, "8 MiB of 256 KiB pieces");
    assert_eq!(readiness.tail.total, 16, "4 MiB of 256 KiB pieces");
    let done = session.stats(hash).unwrap().selected_done_bytes;
    assert!(
        done < EPISODE / 2,
        "the tail arrived without downloading sequentially to it ({} MiB of {})",
        done / MIB,
        EPISODE / MIB
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn without_a_tail_stream_the_tail_waits_for_sequential_download() {
    // Control for the test above: the second stream is what fetches the tail.
    let seeder = seeder(EPISODE, 16 * MIB_PER_S).await;
    let cache = tempfile::tempdir().unwrap();
    let (session, hash) = leecher(cache.path(), seeder.addr, &seeder.torrent).await;
    let _head = session.stream(hash, 0).await.unwrap();

    let readiness = timeout(LIMIT, async {
        loop {
            let r = session.readiness(hash, 0).unwrap();
            if r.head.is_complete() {
                return r;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();

    assert!(
        !readiness.tail.is_complete(),
        "tail not yet fetched: {readiness:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_seek_blocks_until_its_pieces_arrive() {
    let seeder = seeder(EPISODE, 4 * MIB_PER_S).await;
    let cache = tempfile::tempdir().unwrap();
    let (session, hash) = leecher(cache.path(), seeder.addr, &seeder.torrent).await;
    let mut stream = session.stream(hash, 0).await.unwrap();
    let at = 60 * MIB;
    stream.seek(SeekFrom::Start(at)).await.unwrap();
    let mut buf = vec![0; 64 * 1024];

    let first_poll = timeout(Duration::ZERO, stream.read_exact(&mut buf)).await;
    assert!(first_poll.is_err(), "the read waits for a missing piece");

    timeout(LIMIT, stream.read_exact(&mut buf))
        .await
        .unwrap()
        .unwrap();
    let start = usize::try_from(at).unwrap();
    assert!(
        buf == seeder.content[start..start + buf.len()],
        "the read returns the seeded bytes"
    );
    let done = session.stats(hash).unwrap().selected_done_bytes;
    assert!(
        done < at,
        "the seek fetched its piece out of order ({} MiB done)",
        done / MIB
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_partial_stream_deletes_its_files() {
    let seeder = seeder(EPISODE, 4 * MIB_PER_S).await;
    let cache = tempfile::tempdir().unwrap();
    let (session, hash) = leecher(cache.path(), seeder.addr, &seeder.torrent).await;
    let dir = cache.path().join(hash.to_string());
    wait_for(&session, hash, |s| s.selected_done_bytes > 0).await;
    assert!(dir.join("ep.mkv").exists(), "the partial file is on disk");

    session.drop_stream(hash).await.unwrap();

    assert!(!dir.exists(), "the infohash dir is gone");
    assert!(
        matches!(session.stats(hash), Err(Error::NotFound(_))),
        "the torrent left the session"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dropping_a_complete_stream_keeps_it_seeding() {
    let seeder = seeder(2 * MIB, 0).await;
    let cache = tempfile::tempdir().unwrap();
    let (session, hash) = leecher(cache.path(), seeder.addr, &seeder.torrent).await;
    wait_for(&session, hash, |s| s.finished).await;
    // The playback session ended: #179 pauses the torrent, then drops the stream.
    session.pause(hash).await.unwrap();

    session.drop_stream(hash).await.unwrap();

    let file = cache.path().join(hash.to_string()).join("ep.mkv");
    assert!(
        std::fs::read(&file).unwrap() == seeder.content,
        "the complete file is kept"
    );
    assert!(!session.stats(hash).unwrap().paused, "resumed to seed");
    // A third session that only knows the leecher downloads the file from it.
    let other_cache = tempfile::tempdir().unwrap();
    let (other, _) = leecher(
        other_cache.path(),
        session.listen_addr().unwrap(),
        &seeder.torrent,
    )
    .await;
    wait_for(&other, hash, |s| s.finished).await;
    assert!(
        session.stats(hash).unwrap().uploaded_bytes > 0,
        "it uploaded"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quit_deletes_every_torrent_dir() {
    let seeder = seeder(2 * MIB, 0).await;
    let cache = tempfile::tempdir().unwrap();
    let (session, hash) = leecher(cache.path(), seeder.addr, &seeder.torrent).await;
    wait_for(&session, hash, |s| s.finished).await;
    let unrelated = cache.path().join("notes.txt");
    std::fs::write(&unrelated, b"not a torrent").unwrap();

    session.shutdown().await.unwrap();

    assert!(
        !cache.path().join(hash.to_string()).exists(),
        "the seeding torrent's dir is gone"
    );
    assert!(unrelated.exists(), "non-torrent entries are left alone");
}

#[tokio::test]
async fn next_launch_sweeps_orphaned_dirs() {
    let cache = tempfile::tempdir().unwrap();
    let orphan = cache
        .path()
        .join("0123456789abcdef0123456789abcdef01234567");
    std::fs::create_dir_all(orphan.join("Show")).unwrap();
    std::fs::write(orphan.join("Show").join("ep.mkv"), b"partial").unwrap();
    let unrelated = cache.path().join("keep");
    std::fs::create_dir(&unrelated).unwrap();

    let session = TorrentSession::start(
        cache.path().to_path_buf(),
        PeerDiscovery::Only(vec![]),
        TorrentSessionOptions::default(),
    )
    .await
    .unwrap();

    assert!(!orphan.exists(), "the crashed run's torrent dir is swept");
    assert!(unrelated.exists(), "non-infohash dirs are left alone");
    session.shutdown().await.unwrap();
}
