//! The Phase 3 exit criterion as an engine test (#230).
//!
//! The latest aired episode of a show is auto-selected through the real
//! search pipeline (both indexers answered by fixture feeds), verified at
//! file level through the engine's own torrent session, and played in a
//! fake player while it downloads from an in-process seeder. A seek near the
//! end, past the tail window, is served before the download finishes.
//!
//! Nothing here touches the internet. The engine's session only knows the
//! seeder's loopback address (`PeerDiscovery::Only`): the magnet the stream
//! adds carries no tracker (#217), so on real hardware peers come from DHT.

use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    sync::{Arc, Mutex},
    time::Duration,
};

use db::Db;
use domain::{AppEvent, ExternalId, InfoHash, MediaId, MediaKind};
use futures::SinkExt;
use librqbit::{CreateTorrentOptions, ListenerOptions, Session, SessionOptions};
use metadata::CatalogCache;
use player::PlayerEvent;
use reqwest::{StatusCode, header::RANGE};
use search::{Query, SearchRequest, Searcher, Transport};
use settings::{AutoSelectProfile, PlaybackSettings};
use tempfile::TempDir;
use tokio::time::{sleep, timeout};
use torrent::{PeerDiscovery, TorrentSessionOptions};

use crate::{
    AutoSelectProgress, AutoSelectRequest, AutoSelection, AutoSelector, BoxFuture, EpisodeIds,
    LocalProgressSync, PlayRequest, PlaybackManager, SessionFiles, TorrentStreams,
    playback::fakes::FakeLauncher,
};

const FRIEREN: u32 = 154_587;
/// The latest aired episode, the one the Entry Torrent tab auto-selects.
const AIRED: u32 = 5;
const RELEASE: &str = "[SubsPlease] Sousou no Frieren - 05 (1080p) [5E1A0C05].mkv";

const MIB: u64 = 1024 * 1024;
const PIECE: u32 = 256 * 1024;
const EPISODE_BYTES: u64 = 96 * MIB;
/// One episode's length as the fake player reports it, in seconds.
const EPISODE_SECONDS: f64 = 1420.0;
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

/// A loopback librqbit session seeding [`RELEASE`].
struct Seeder {
    session: Arc<Session>,
    _dir: TempDir,
    addr: SocketAddr,
    info_hash: InfoHash,
    content: Vec<u8>,
}

impl Seeder {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(RELEASE);
        let content = contents(EPISODE_BYTES);
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
        let seeder = Self {
            addr: session.listen_addr().unwrap(),
            info_hash: InfoHash::from_bytes(created.info_hash().0),
            session,
            _dir: dir,
            content,
        };
        // Fast enough for the readiness gate, slow enough that the whole
        // file takes far longer than the test's reads.
        seeder.throttle(4 * 1024 * 1024);
        seeder
    }

    fn throttle(&self, bytes_per_sec: u32) {
        self.session
            .ratelimits
            .set_upload_bps(NonZeroU32::new(bytes_per_sec));
    }
}

/// Nyaa's RSS feed: the seeded release, a 720p release of the same episode
/// that ranks below it, and the previous episode, which the filters drop.
/// Only the seeded release has peers.
fn nyaa_rss(seeded: InfoHash) -> String {
    let item = |title: &str, hash: &str, seeders: u32| {
        format!(
            "<item><title>{title}</title>\
             <link>https://nyaa.si/download/1.torrent</link>\
             <guid>https://nyaa.si/view/1</guid>\
             <pubDate>Fri, 27 Nov 2026 16:00:00 +0000</pubDate>\
             <nyaa:seeders>{seeders}</nyaa:seeders><nyaa:leechers>4</nyaa:leechers>\
             <nyaa:downloads>900</nyaa:downloads><nyaa:infoHash>{hash}</nyaa:infoHash>\
             <nyaa:categoryId>1_2</nyaa:categoryId>\
             <nyaa:category>Anime - English-translated</nyaa:category>\
             <nyaa:size>96.0 MiB</nyaa:size><nyaa:comments>0</nyaa:comments>\
             <nyaa:trusted>Yes</nyaa:trusted><nyaa:remake>No</nyaa:remake></item>"
        )
    };
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\
         <rss version=\"2.0\" xmlns:nyaa=\"https://nyaa.si/xmlns/nyaa\"><channel>\
         <title>Nyaa.si</title>{}{}{}</channel></rss>",
        item(RELEASE.trim_end_matches(".mkv"), &seeded.to_string(), 150),
        item(
            "[OldGroup] Sousou no Frieren - 05 (720p) [DEADBEEF]",
            &"b".repeat(40),
            3
        ),
        item(
            "[SubsPlease] Sousou no Frieren - 04 (1080p) [5E1A0C04]",
            &"c".repeat(40),
            400
        ),
    )
}

/// `AnimeTosho`'s JSON feed: the seeded release again, which the searcher
/// dedupes by infohash.
fn tosho_json(seeded: InfoHash) -> String {
    format!(
        r#"[{{"title":"{}","info_hash":"{seeded}","seeders":140,"leechers":4,"total_size":{EPISODE_BYTES},"is_seadex":false,"anidb_aid":17617,"num_files":1}}]"#,
        RELEASE.trim_end_matches(".mkv"),
    )
}

/// Both indexers, answered from fixture feeds by query kind.
#[derive(Clone)]
struct FixtureIndexers {
    nyaa: String,
    tosho: String,
}

impl Transport for FixtureIndexers {
    fn fetch(&self, query: &Query) -> impl Future<Output = Result<Vec<u8>, search::Error>> + Send {
        let body = if matches!(query, Query::Nyaa { .. }) {
            &self.nyaa
        } else {
            &self.tosho
        };
        std::future::ready(Ok(body.clone().into_bytes()))
    }
}

struct NoIds;

impl EpisodeIds for NoIds {
    fn anidb_eid(&self, _anilist_id: u32, _episode_key: String) -> BoxFuture<Option<u32>> {
        Box::pin(async { None })
    }
}

struct Fixture {
    _dir: TempDir,
    db: Db,
    streams: Arc<TorrentStreams>,
    selector: AutoSelector,
    manager: Arc<PlaybackManager>,
    launcher: FakeLauncher,
    /// Kept open so the manager's events have a receiver.
    _events: flume::Receiver<AppEvent>,
}

impl Fixture {
    /// An engine whose torrent session only knows `peer` and whose
    /// auto-select searches `indexers`.
    async fn new(peer: SocketAddr, indexers: FixtureIndexers) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        let cache = dir.path().join("cache");
        std::fs::create_dir_all(&cache).unwrap();
        let client = net::Client::builder().max_retries(0).build().unwrap();
        let anilist = metadata::AniList::with_endpoint(client, "http://127.0.0.1:1/");
        let catalog = CatalogCache::new(anilist, net::PosterCache::new(&cache).unwrap());
        let streams = Arc::new(TorrentStreams::new(
            dir.path().join("torrents"),
            PeerDiscovery::Only(vec![peer]),
            TorrentSessionOptions::default(),
        ));
        let selector = AutoSelector::new(
            Arc::new(Arc::new(Searcher::new(indexers))),
            Arc::new(SessionFiles::new(Arc::clone(&streams))),
            Arc::new(NoIds),
        );
        let (events, receiver) = flume::unbounded();
        let launcher = FakeLauncher::default();
        let manager = PlaybackManager::with_launcher(
            db.clone(),
            catalog,
            Arc::new(LocalProgressSync::new(db.clone())),
            events,
            &PlaybackSettings::default(),
            cache,
            Arc::new(launcher.clone()),
        )
        .with_torrent(Arc::clone(&streams));
        Self {
            _dir: dir,
            db,
            streams,
            selector,
            manager: Arc::new(manager),
            launcher,
            _events: receiver,
        }
    }

    async fn media(&self) -> MediaId {
        self.db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(FRIEREN)])
            .await
            .unwrap()
            .media
    }

    /// Auto-selects [`AIRED`] with the default profile, as the Entry Torrent
    /// tab does, and returns the choice with every progress report.
    async fn auto_select(&self) -> (AutoSelection, Vec<AutoSelectProgress>) {
        let reports = Arc::new(Mutex::new(Vec::new()));
        let selection = timeout(
            LIMIT,
            self.selector.select_with_progress(
                AutoSelectRequest::new(
                    SearchRequest::new(vec!["Sousou no Frieren".to_owned()], 1, AIRED),
                    Some(FRIEREN),
                ),
                &AutoSelectProfile::default(),
                {
                    let reports = Arc::clone(&reports);
                    move |report| reports.lock().unwrap().push(report)
                },
            ),
        )
        .await
        .expect("auto-select finishes")
        .expect("auto-select picks a release");
        let reports = reports.lock().unwrap().clone();
        (selection, reports)
    }

    async fn progress(&self, media: MediaId) -> Option<u32> {
        self.db
            .local_list()
            .entry(media)
            .await
            .unwrap()
            .map(|entry| entry.progress)
    }
}

/// Reads `start..=end` of `url` through the relay and checks it against the
/// seeded `content`.
async fn read_matches(url: &str, content: &[u8], start: u64, end: u64) {
    let response = reqwest::Client::new()
        .get(url)
        .header(RANGE, format!("bytes={start}-{end}"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
    let range = usize::try_from(start).unwrap()..=usize::try_from(end).unwrap();
    assert!(
        response.bytes().await.unwrap().as_ref() == content.get(range).unwrap(),
        "bytes {start}-{end} differ from the seeded file"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn auto_select_plays_while_downloading_and_a_seek_near_the_end_is_served() {
    let seeder = Seeder::new().await;
    let fixture = Fixture::new(
        seeder.addr,
        FixtureIndexers {
            nyaa: nyaa_rss(seeder.info_hash),
            tosho: tosho_json(seeder.info_hash),
        },
    )
    .await;
    let media = fixture.media().await;

    // Search both indexers, rank, list the top candidate's files from the
    // seeder and pick the episode's file.
    let (selection, reports) = fixture.auto_select().await;
    assert_eq!(
        reports,
        [
            AutoSelectProgress::Searching,
            AutoSelectProgress::Verifying {
                checking: 1,
                total: 2
            },
        ],
        "the 1080p release ranks first and verifies"
    );
    assert_eq!(selection.torrent.infohash, seeder.info_hash.to_string());
    assert_eq!(selection.file_path, RELEASE);
    assert_eq!(
        fixture.streams.active(),
        [],
        "listing the files adds nothing"
    );

    // Play it, as the Entry Torrent tab does with the selection's source.
    fixture
        .manager
        .play(PlayRequest::new(media, AIRED, selection.source.clone()))
        .await
        .unwrap();
    timeout(LIMIT, fixture.launcher.wait_for_launch())
        .await
        .expect("the player opens once the readiness gate is met");
    let launch = fixture.launcher.launched().pop().unwrap();
    assert_eq!(launch.source, selection.source);
    let url = launch.stream_url.expect("the player opens a relay URL");
    let mut player = fixture.launcher.sender(0);
    player
        .send(PlayerEvent::MediaReady {
            path: None,
            duration: Some(EPISODE_SECONDS),
        })
        .await
        .unwrap();
    let session = fixture.streams.session().await.unwrap();
    let at_launch = session.stats(seeder.info_hash).unwrap();
    assert!(
        !at_launch.finished && at_launch.selected_done_bytes < at_launch.selected_bytes,
        "playback started while downloading: {at_launch:?}"
    );

    // From here the seeder is slow: the rest of the file would take minutes.
    seeder.throttle(512 * 1024);
    read_matches(&url, &seeder.content, 0, 63).await;

    // The player seeks to 98 %: 6 MiB before the end, past the 4 MiB tail
    // window, so the relay waits for pieces the download has not reached.
    player
        .send(PlayerEvent::Position {
            seconds: EPISODE_SECONDS * 0.98,
        })
        .await
        .unwrap();
    let near_end = EPISODE_BYTES - 6 * MIB;
    timeout(
        LIMIT,
        read_matches(&url, &seeder.content, near_end, near_end + 64 * 1024 - 1),
    )
    .await
    .expect("the seek is served");
    let after_seek = session.stats(seeder.info_hash).unwrap();
    assert!(
        !after_seek.finished,
        "the seek was served before the download finished: {after_seek:?}"
    );

    // Playing on to the end counts the episode on the Local platform.
    read_matches(&url, &seeder.content, EPISODE_BYTES - 64, EPISODE_BYTES - 1).await;
    player
        .send(PlayerEvent::Position {
            seconds: EPISODE_SECONDS - 1.0,
        })
        .await
        .unwrap();
    player.send(PlayerEvent::Eof).await.unwrap();
    timeout(LIMIT, async {
        while fixture.progress(media).await != Some(AIRED) {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the episode is marked watched");

    fixture.manager.stop().await;
    assert_eq!(
        fixture.streams.active(),
        [],
        "the stream ends with playback"
    );
}
