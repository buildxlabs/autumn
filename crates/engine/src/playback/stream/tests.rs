//! Torrent playback end to end: an in-process librqbit seeder, the engine's
//! own session and relay, and a fake player. No internet.

use std::{
    net::{Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    path::PathBuf,
    sync::{Arc, RwLock},
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

use search::{Indexer, SearchRequest, Torrent};
use settings::AutoSelectSettings;
use torrent::{TorrentFile, TorrentSource};

use crate::{
    AutoSelectRequest, AutoSelector, BoxFuture, EpisodeAiring, EpisodeIds, FileListError,
    FileLister, LocalProgressSync, PlayRequest, PlaybackManager, StreamOrigin, TorrentSearch,
    TorrentStreams, playback::fakes::FakeLauncher,
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
        Self::build(peer, no_peers_after, None).await
    }

    /// A manager that preloads, whose auto-select finds `results`, every
    /// episode aired, a 10 ms countdown step and a `grace` grace period.
    async fn preloading(peer: SocketAddr, results: Vec<Torrent>, grace: Duration) -> Self {
        Self::build(peer, Duration::from_secs(60), Some((results, grace))).await
    }

    async fn build(
        peer: SocketAddr,
        no_peers_after: Duration,
        preload: Option<(Vec<Torrent>, Duration)>,
    ) -> Self {
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
        if let Some((results, grace)) = preload {
            let selector = AutoSelector::new(
                Arc::new(FakeSearch(results)),
                Arc::new(NoFiles),
                Arc::new(NoIds),
            );
            manager = manager.with_preload(
                Arc::new(RwLock::new(selector)),
                Arc::new(AllAired),
                AutoSelectSettings::default(),
            );
            manager.countdown_step = Duration::from_millis(10);
            manager.preload_grace = grace;
        }
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

// Next-episode preload (#224, #208).

/// Auto-select's indexers, answering every search with the same results.
struct FakeSearch(Vec<Torrent>);

impl TorrentSearch for FakeSearch {
    fn search(&self, _request: SearchRequest) -> BoxFuture<Result<Vec<Torrent>, search::Error>> {
        let results = self.0.clone();
        Box::pin(async move { Ok(results) })
    }
}

/// Every result carries its file list, so none is fetched.
struct NoFiles;

impl FileLister for NoFiles {
    fn files(&self, _torrent: Torrent) -> BoxFuture<Result<Vec<TorrentFile>, FileListError>> {
        Box::pin(async { Err(FileListError::TimedOut) })
    }
}

struct NoIds;

impl EpisodeIds for NoIds {
    fn anidb_eid(&self, _anilist_id: u32, _episode_key: String) -> BoxFuture<Option<u32>> {
        Box::pin(async { None })
    }
}

struct AllAired;

impl EpisodeAiring for AllAired {
    fn has_aired(&self, _anilist_id: u32, _episode: u32) -> BoxFuture<bool> {
        Box::pin(async { true })
    }
}

/// One loopback librqbit session seeding several torrents, slowly enough
/// that a preload is still partial when a test drops it.
struct Swarm {
    session: Arc<Session>,
    dir: TempDir,
    addr: SocketAddr,
}

impl Swarm {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
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
        session
            .ratelimits
            .set_upload_bps(NonZeroU32::new(8 * 1024 * 1024));
        Self {
            addr: session.listen_addr().unwrap(),
            session,
            dir,
        }
    }

    /// Seeds `files` of `EPISODE` bytes each: a single-file torrent for one
    /// name, else a batch folder `folder`.
    async fn seed(&self, folder: &str, files: &[&str]) -> InfoHash {
        let path = if let [single] = files {
            self.dir.path().join(single)
        } else {
            self.dir.path().join(folder)
        };
        for name in files {
            let file = if files.len() == 1 {
                path.clone()
            } else {
                path.join(name)
            };
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, contents(EPISODE)).unwrap();
        }
        let (created, handle) = self
            .session
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
        InfoHash::from_bytes(created.info_hash().0)
    }
}

const EP1: &str = "[Grp] Show - 01 [1080p].mkv";
const EP2: &str = "[Grp] Show - 02 [1080p].mkv";

/// The indexer result of a single-file torrent holding `file`.
fn result(info_hash: InfoHash, file: &str) -> Torrent {
    let mut torrent = Torrent::new(
        Indexer::Nyaa,
        info_hash.to_string(),
        file.trim_end_matches(".mkv"),
    );
    torrent.seeders = Some(10);
    torrent.size = Some(EPISODE);
    torrent.magnet_url = Some(format!("magnet:?xt=urn:btih:{info_hash}"));
    torrent.files = vec![file.to_owned()];
    torrent
}

/// Episode 1 found by an auto-select search for "Show".
fn episode_1(media: MediaId, source: StreamSource) -> PlayRequest {
    PlayRequest::new(media, 1, source).with_auto_select(AutoSelectRequest::new(
        SearchRequest::new(vec!["Show".to_owned()], 1, 1),
        None,
    ))
}

impl Fixture {
    /// Plays `request`, then has the player report a 100 s file at `at`.
    async fn play_at(
        &self,
        request: PlayRequest,
        at: f64,
    ) -> futures::channel::mpsc::UnboundedSender<PlayerEvent> {
        let launches = self.launcher.launches();
        self.manager.play(request).await.unwrap();
        wait_until("the launch", || self.launcher.launches() > launches).await;
        let mut player = self.launcher.sender(launches);
        player
            .send(PlayerEvent::MediaReady {
                path: None,
                duration: Some(100.0),
            })
            .await
            .unwrap();
        player
            .send(PlayerEvent::Position { seconds: at })
            .await
            .unwrap();
        player
    }

    async fn preloaded(&self, source: StreamSource) {
        wait_until("the preload", || {
            self.manager.preloaded().map(|preloaded| preloaded.source) == Some(source.clone())
        })
        .await;
    }

    fn active(&self) -> Vec<InfoHash> {
        let mut active = self.streams.active();
        active.sort_unstable();
        active
    }

    /// Plays to EOF within the last 2 %.
    async fn finish(player: &mut futures::channel::mpsc::UnboundedSender<PlayerEvent>) {
        player
            .send(PlayerEvent::Position { seconds: 99.0 })
            .await
            .unwrap();
        player.send(PlayerEvent::Eof).await.unwrap();
    }
}

fn sorted(mut hashes: Vec<InfoHash>) -> Vec<InfoHash> {
    hashes.sort_unstable();
    hashes
}

/// Two single-episode torrents, and a fixture whose auto-select finds the
/// second for episode 2.
async fn two_releases(grace: Duration) -> (Swarm, Fixture, InfoHash, InfoHash) {
    let swarm = Swarm::new().await;
    let first = swarm.seed("", &[EP1]).await;
    let second = swarm.seed("", &[EP2]).await;
    let fixture = Fixture::preloading(swarm.addr, vec![result(second, EP2)], grace).await;
    (swarm, fixture, first, second)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_preload_starts_at_half_way_counts_as_active_and_a_stop_drops_it() {
    let (_swarm, fixture, first, second) = two_releases(Duration::from_secs(600)).await;
    let media = fixture.media().await;

    let mut player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 49.0)
        .await;
    sleep(Duration::from_millis(300)).await;
    assert_eq!(fixture.manager.preloaded(), None, "nothing before 50 %");
    // A seek past the middle counts.
    player
        .send(PlayerEvent::Position { seconds: 75.0 })
        .await
        .unwrap();
    fixture.preloaded(StreamSource::torrent(second, 0)).await;

    assert_eq!(
        fixture.active(),
        sorted(vec![first, second]),
        "Clear keeps both"
    );
    assert_eq!(fixture.streams.origin(second), Some(StreamOrigin::Preload));
    assert_eq!(fixture.streams.origin(first), Some(StreamOrigin::Playback));
    assert_eq!(
        fixture
            .manager
            .preloaded()
            .map(|preloaded| preloaded.episode),
        Some(2)
    );

    fixture.manager.stop().await;
    assert_eq!(fixture.active(), [], "Stop drops the preload now");
    assert!(
        !fixture.root.join(second.to_string()).exists(),
        "a partial preload is deleted"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_next_consumes_a_same_batch_preload_without_dropping_the_batch() {
    let swarm = Swarm::new().await;
    let batch = swarm
        .seed("[Grp] Show (01-02)", &["Show - 01.mkv", "Show - 02.mkv"])
        .await;
    let fixture = Fixture::preloading(swarm.addr, Vec::new(), Duration::from_secs(600)).await;
    let media = fixture.media().await;
    let files = fixture
        .streams
        .session()
        .await
        .unwrap()
        .list_files(TorrentSource::Magnet(format!(
            "magnet:?xt=urn:btih:{batch}"
        )))
        .await
        .unwrap();
    let index = |name: &str| {
        files
            .iter()
            .find(|file| file.path.ends_with(name))
            .unwrap()
            .index
    };
    let (one, two) = (index("Show - 01.mkv"), index("Show - 02.mkv"));

    let mut player = fixture
        .play_at(
            PlayRequest::new(media, 1, StreamSource::torrent(batch, one)),
            60.0,
        )
        .await;
    // The same batch holds episode 2: no search.
    fixture.preloaded(StreamSource::torrent(batch, two)).await;
    assert_eq!(fixture.active(), [batch]);
    let session = fixture.streams.session().await.unwrap();
    wait_until("episode 2's readiness gate", || {
        session
            .readiness(batch, two)
            .is_ok_and(torrent::Readiness::is_ready)
    })
    .await;
    // From here a dropped and re-added batch could never meet the gate again.
    swarm.session.ratelimits.set_upload_bps(NonZeroU32::new(1));

    Fixture::finish(&mut player).await;
    wait_until("the switch to episode 2", || {
        fixture
            .launcher
            .loads()
            .last()
            .is_some_and(|url| url.to_string_lossy().ends_with(&format!("/{batch}/{two}")))
    })
    .await;

    assert_eq!(fixture.manager.preloaded(), None, "consumed");
    assert_eq!(fixture.active(), [batch]);
    assert_eq!(fixture.streams.origin(batch), Some(StreamOrigin::Playback));
    assert!(
        fixture.root.join(batch.to_string()).exists(),
        "ending episode 1's stream left the batch to episode 2"
    );
    let url = fixture
        .launcher
        .loads()
        .last()
        .unwrap()
        .to_string_lossy()
        .into_owned();
    assert_eq!(
        get(&url, "bytes=0-15").await.status(),
        StatusCode::PARTIAL_CONTENT
    );
    assert_eq!(fixture.watched_progress(media).await, Some(1));
    fixture.manager.stop().await;
    assert_eq!(fixture.active(), []);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_next_takes_the_library_file_first_and_ends_the_stream() {
    let (_swarm, fixture, first, _second) = two_releases(Duration::from_secs(600)).await;
    let media = fixture.media().await;
    let mut files = fixture.db.local_file().all().await.unwrap();
    files.push(db::LocalFile {
        path: PathBuf::from("/library/Show - 02.mkv"),
        size: 0,
        modified: 0,
        title: None,
        group: None,
        season: None,
        episode: Some(2),
        kind: "episode".to_owned(),
        media_id: Some(media),
        confidence: Some(1.0),
        locked: false,
        ignored: false,
        filler: false,
        shelved: false,
    });
    fixture.db.local_file().sync(&files).await.unwrap();

    let mut player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    sleep(Duration::from_millis(500)).await;
    assert_eq!(
        fixture.manager.preloaded(),
        None,
        "a library episode is not preloaded"
    );
    assert_eq!(fixture.active(), [first]);

    Fixture::finish(&mut player).await;
    wait_until("the switch to the library file", || {
        fixture.launcher.loads() == [PathBuf::from("/library/Show - 02.mkv")]
    })
    .await;
    wait_until("the stream to end", || fixture.streams.active().is_empty()).await;
    fixture.manager.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn playing_the_preloaded_episode_from_anywhere_consumes_it() {
    let (_swarm, fixture, first, second) = two_releases(Duration::from_secs(600)).await;
    let media = fixture.media().await;
    let _player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    fixture.preloaded(StreamSource::torrent(second, 0)).await;

    // The screen asked for some other release of episode 2.
    let other: InfoHash = "e".repeat(40).parse().unwrap();
    fixture
        .manager
        .play(PlayRequest::new(media, 2, StreamSource::torrent(other, 3)))
        .await
        .unwrap();
    wait_until("the second launch", || fixture.launcher.launches() == 2).await;

    assert_eq!(
        fixture.launcher.launched().last().unwrap().source,
        StreamSource::torrent(second, 0),
        "the preload's stream plays"
    );
    assert_eq!(fixture.manager.preloaded(), None);
    assert_eq!(fixture.active(), [second], "episode 1's stream ended");
    assert_eq!(fixture.streams.origin(second), Some(StreamOrigin::Playback));
    fixture.manager.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn closing_the_player_keeps_the_preload_for_the_grace_period() {
    let (_swarm, fixture, first, second) = two_releases(Duration::from_millis(800)).await;
    let media = fixture.media().await;
    let mut player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    fixture.preloaded(StreamSource::torrent(second, 0)).await;

    player.send(PlayerEvent::Exited).await.unwrap();
    wait_until("the session to end", || {
        fixture.streams.origin(first).is_none()
    })
    .await;
    assert_eq!(fixture.active(), [second], "kept, still downloading");
    assert!(fixture.manager.preloaded().is_some());

    wait_until("the grace to end", || fixture.manager.preloaded().is_none()).await;
    wait_until("the drop", || fixture.streams.active().is_empty()).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancelling_auto_next_keeps_the_preload_for_the_grace_period() {
    let (_swarm, mut fixture, first, second) = two_releases(Duration::from_millis(800)).await;
    // A countdown long enough to cancel.
    Arc::get_mut(&mut fixture.manager).unwrap().countdown_step = Duration::from_secs(5);
    let media = fixture.media().await;
    let mut player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    fixture.preloaded(StreamSource::torrent(second, 0)).await;

    Fixture::finish(&mut player).await;
    fixture
        .next("the countdown to episode 2", |event| match event {
            AppEvent::PlaybackAutoNext(Some(next)) if next.episode == 2 => Some(()),
            _ => None,
        })
        .await;
    assert!(fixture.manager.cancel_auto_next());

    sleep(Duration::from_millis(200)).await;
    assert!(fixture.manager.preloaded().is_some(), "kept for now");
    wait_until("the grace to end", || fixture.manager.preloaded().is_none()).await;
    wait_until("the drop", || fixture.active() == [first]).await;
    fixture.manager.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_play_of_another_episode_drops_the_preload_now() {
    let (_swarm, fixture, first, second) = two_releases(Duration::from_secs(600)).await;
    let media = fixture.media().await;
    let _player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    fixture.preloaded(StreamSource::torrent(second, 0)).await;

    fixture
        .manager
        .play(PlayRequest::new(
            media,
            5,
            StreamSource::local("/library/Show - 05.mkv"),
        ))
        .await
        .unwrap();

    assert_eq!(fixture.manager.preloaded(), None);
    assert_eq!(fixture.active(), []);
    fixture.manager.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn turning_the_toggle_off_drops_the_preload_now() {
    let (_swarm, fixture, first, second) = two_releases(Duration::from_secs(600)).await;
    let media = fixture.media().await;
    let _player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    fixture.preloaded(StreamSource::torrent(second, 0)).await;

    fixture.manager.set_preload(false).await;

    assert_eq!(fixture.manager.preloaded(), None);
    assert_eq!(fixture.active(), [first], "the current stream plays on");
    fixture.manager.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_failed_preload_shows_only_at_episode_end() {
    let swarm = Swarm::new().await;
    let first = swarm.seed("", &[EP1]).await;
    let fixture = Fixture::preloading(swarm.addr, Vec::new(), Duration::from_secs(600)).await;
    let media = fixture.media().await;
    let mut player = fixture
        .play_at(episode_1(media, StreamSource::torrent(first, 0)), 60.0)
        .await;
    sleep(Duration::from_millis(500)).await;
    assert!(
        !fixture
            .events
            .drain()
            .any(|event| matches!(event, AppEvent::PlaybackNoRelease(Some(_)))),
        "nothing shows during playback"
    );

    Fixture::finish(&mut player).await;
    let notice = fixture
        .next("the no-release notice", |event| match event {
            AppEvent::PlaybackNoRelease(Some(notice)) => Some(notice),
            _ => None,
        })
        .await;

    assert_eq!((notice.media, notice.episode), (media, 2));
    assert!(
        notice.reason.contains("no torrent matches"),
        "{}",
        notice.reason
    );
    fixture.manager.stop().await;
}
