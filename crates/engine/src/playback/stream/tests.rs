//! Torrent playback end to end: an in-process librqbit seeder, the engine's
//! own session and relay, and a fake player. No internet.

use std::{
    net::{Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    path::PathBuf,
    sync::Arc,
    time::Duration,
};

use db::Db;
use domain::{
    AppEvent, ExternalId, InfoHash, MediaId, MediaKind, PrepareStage, Preparing, StreamSource,
};
use futures::SinkExt;
use librqbit::{CreateTorrentOptions, ListenerOptions, Session, SessionOptions};
use metadata::CatalogCache;
use player::PlayerEvent;
use reqwest::{StatusCode, header::RANGE};
use settings::PlaybackSettings;
use tempfile::TempDir;
use tokio::time::{sleep, timeout};
use torrent::{PeerDiscovery, TorrentSessionOptions};

use crate::{
    LocalProgressSync, PlayRequest, PlaybackManager, TorrentStreams, playback::fakes::FakeLauncher,
};

const MIB: u64 = 1024 * 1024;
const PIECE: u32 = 256 * 1024;
/// Big enough that the middle of the file is far from both windows.
const EPISODE: u64 = 96 * MIB;
const LIMIT: Duration = Duration::from_secs(60);

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
    session: Arc<Session>,
    _dir: TempDir,
    addr: SocketAddr,
    info_hash: InfoHash,
    content: Vec<u8>,
}

impl Seeder {
    /// Limits how fast the seeder uploads, `None` for unlimited.
    fn throttle(&self, bytes_per_sec: Option<u32>) {
        self.session
            .ratelimits
            .set_upload_bps(bytes_per_sec.and_then(NonZeroU32::new));
    }
}

/// A plain librqbit session seeding one file, `ep.mkv`, over loopback.
async fn seeder(len: u64) -> Seeder {
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
    let seeder = Seeder {
        addr: session.listen_addr().unwrap(),
        info_hash: InfoHash::from_bytes(created.info_hash().0),
        session,
        _dir: dir,
        content,
    };
    // Slow enough that the readiness gate is met long before the middle.
    seeder.throttle(Some(8 * 1024 * 1024));
    seeder
}

struct Fixture {
    _dir: TempDir,
    root: PathBuf,
    db: Db,
    manager: Arc<PlaybackManager>,
    streams: Arc<TorrentStreams>,
    launcher: FakeLauncher,
    events: flume::Receiver<AppEvent>,
}

impl Fixture {
    /// A manager whose torrent session only knows `peer`. A threshold of
    /// 100 % leaves EOF as the only way to mark an episode watched.
    async fn new(peer: SocketAddr, no_peers_after: Duration) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let client = net::Client::builder().max_retries(0).build().unwrap();
        let anilist = metadata::AniList::with_endpoint(client, "http://127.0.0.1:1/");
        let catalog = CatalogCache::new(anilist, net::PosterCache::new(&cache).unwrap());
        let root = dir.path().join("torrents");
        let streams = Arc::new(TorrentStreams::new(
            root.clone(),
            PeerDiscovery::Only(vec![peer]),
            TorrentSessionOptions::default(),
        ));
        let (events, receiver) = flume::unbounded();
        let launcher = FakeLauncher::default();
        let settings = PlaybackSettings {
            watched_threshold: 100,
            ..PlaybackSettings::default()
        };
        let mut manager = PlaybackManager::with_launcher(
            db.clone(),
            catalog,
            Arc::new(LocalProgressSync::new(db.clone())),
            events,
            &settings,
            cache,
            Arc::new(launcher.clone()),
        )
        .with_torrent(Arc::clone(&streams));
        manager.no_peers_after = no_peers_after;
        Self {
            _dir: dir,
            root,
            db,
            manager: Arc::new(manager),
            streams,
            launcher,
            events: receiver,
        }
    }

    async fn media(&self) -> MediaId {
        self.db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
            .await
            .unwrap()
            .media
    }

    /// Waits for the next event that `pick` accepts, skipping the others.
    async fn next<T>(&self, what: &str, pick: impl Fn(AppEvent) -> Option<T>) -> T {
        let found = timeout(LIMIT, async {
            loop {
                if let Some(found) = pick(self.events.recv_async().await.unwrap()) {
                    return found;
                }
            }
        })
        .await;
        assert!(found.is_ok(), "timed out waiting for {what}");
        found.unwrap()
    }

    /// Every `Preparing` report up to the one that ends the phase.
    async fn preparing(&self) -> Vec<Preparing> {
        let mut reports = Vec::new();
        loop {
            match self
                .next("a preparing report", |event| match event {
                    AppEvent::PlaybackPreparing(report) => Some(report),
                    _ => None,
                })
                .await
            {
                Some(report) => reports.push(report),
                None => return reports,
            }
        }
    }

    async fn watched_progress(&self, media: MediaId) -> Option<u32> {
        self.db
            .local_list()
            .entry(media)
            .await
            .unwrap()
            .map(|entry| entry.progress)
    }
}

async fn wait_until(what: &str, done: impl Fn() -> bool) {
    let waited = timeout(LIMIT, async {
        while !done() {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    assert!(waited.is_ok(), "timed out waiting for {what}");
}

async fn get(url: &str, range: &str) -> reqwest::Response {
    reqwest::Client::new()
        .get(url)
        .header(RANGE, range)
        .send()
        .await
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_torrent_stream_prepares_plays_buffers_and_ends_cleanly() {
    let seeder = seeder(EPISODE).await;
    let fixture = Fixture::new(seeder.addr, Duration::from_secs(60)).await;
    let media = fixture.media().await;
    fixture
        .db
        .continuity()
        .save(media, 1, 40.0, Some(100.0))
        .await
        .unwrap();
    let source = StreamSource::torrent(seeder.info_hash, 0);

    fixture
        .manager
        .play(PlayRequest::new(media, 1, source.clone()))
        .await
        .unwrap();

    // Preparing → Running: reports while the gate fills, then the launch.
    let reports = fixture.preparing().await;
    assert_eq!(
        reports.first().map(|report| report.stage),
        Some(PrepareStage::Metadata),
        "the phase starts waiting for metadata"
    );
    let last = reports.last().expect("at least one report");
    assert_eq!(
        (last.head.total, last.tail.total),
        (32, 16),
        "8 MiB head and 4 MiB tail of 256 KiB pieces"
    );
    assert!(
        reports
            .iter()
            .any(|report| report.stage == PrepareStage::Buffering && report.peers > 0),
        "buffering with the seeder connected: {reports:?}"
    );
    timeout(LIMIT, fixture.launcher.wait_for_launch())
        .await
        .unwrap();
    let launch = fixture.launcher.launched().pop().unwrap();
    assert_eq!(launch.source, source);
    assert!(launch.subtitles.is_empty(), "no sidecar subtitles");
    let url = launch.stream_url.expect("the player opens a relay URL");
    assert!(
        url.starts_with("http://127.0.0.1:") && url.ends_with(&format!("/{}/0", seeder.info_hash)),
        "{url}"
    );
    let head = get(&url, "bytes=0-15").await;
    assert_eq!(head.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        head.bytes().await.unwrap().as_ref(),
        seeder.content.get(..16).unwrap()
    );
    assert_eq!(fixture.streams.active(), [seeder.info_hash]);

    // The saved position resumes once the player knows the duration.
    let mut player = fixture.launcher.sender(0);
    player
        .send(PlayerEvent::MediaReady {
            path: None,
            duration: Some(100.0),
        })
        .await
        .unwrap();
    wait_until("the resume seek", || fixture.launcher.seeks() == [40.0]).await;

    // A read of missing pieces stalls past 2 s: the session is Buffering.
    seeder.throttle(Some(32 * 1024));
    let middle = 60 * MIB;
    let stalled = tokio::spawn({
        let url = url.clone();
        async move { get(&url, &format!("bytes={middle}-{}", middle + 63)).await }
    });
    fixture
        .next("the Buffering flag", |event| {
            (event == AppEvent::PlaybackBuffering(true)).then_some(())
        })
        .await;

    // An EOF mid-file is a stream error: not watched.
    player
        .send(PlayerEvent::Position { seconds: 50.0 })
        .await
        .unwrap();
    player.send(PlayerEvent::Eof).await.unwrap();
    sleep(Duration::from_millis(200)).await;
    assert_eq!(fixture.watched_progress(media).await, None);
    // Within the last 2 % it ends the episode.
    player
        .send(PlayerEvent::Position { seconds: 98.5 })
        .await
        .unwrap();
    player.send(PlayerEvent::Eof).await.unwrap();
    wait_until_async(|| async { fixture.watched_progress(media).await == Some(1) }).await;

    // Session end: token revoked, reader dropped, partial torrent deleted.
    fixture.manager.stop().await;
    stalled.abort();
    assert_eq!(
        get(&url, "bytes=0-15").await.status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(fixture.streams.active(), []);
    assert!(
        !fixture.root.join(seeder.info_hash.to_string()).exists(),
        "a partial stream is deleted when it ends"
    );
}

async fn wait_until_async<F: std::future::Future<Output = bool>>(done: impl Fn() -> F) {
    timeout(LIMIT, async {
        while !done().await {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_stop_while_preparing_drops_the_request_and_the_torrent() {
    // A peer that accepts connections but never speaks: no metadata arrives.
    let silent = tokio::net::TcpListener::bind((Ipv4Addr::LOCALHOST, 0))
        .await
        .unwrap();
    let fixture = Fixture::new(silent.local_addr().unwrap(), Duration::from_millis(100)).await;
    let media = fixture.media().await;
    let info_hash: InfoHash = "1".repeat(40).parse().unwrap();

    fixture
        .manager
        .play(PlayRequest::new(
            media,
            1,
            StreamSource::torrent(info_hash, 0),
        ))
        .await
        .unwrap();
    let report = fixture
        .next("the no-peers hint", |event| match event {
            AppEvent::PlaybackPreparing(Some(report)) if report.no_peers => Some(report),
            _ => None,
        })
        .await;
    assert_eq!(report.stage, PrepareStage::Metadata);

    timeout(Duration::from_secs(5), fixture.manager.stop())
        .await
        .expect("a stop never waits on peers");

    let rest: Vec<AppEvent> = fixture.events.drain().collect();
    assert!(
        rest.contains(&AppEvent::PlaybackPreparing(None)),
        "{rest:?}"
    );
    assert!(
        rest.contains(&AppEvent::PlaybackStopped { unexpected: false }),
        "{rest:?}"
    );
    assert_eq!(fixture.launcher.launches(), 0, "no player opens");
    assert_eq!(fixture.streams.active(), []);
    assert!(!fixture.root.join(info_hash.to_string()).exists());
}
