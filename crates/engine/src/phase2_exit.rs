//! The Phase 2 exit criterion as an engine test (#139).
//!
//! A temp folder of fansub releases is scanned against a fake candidate
//! source, an episode is played through a fake player, stopped midway,
//! resumed after media-ready and watched past the threshold, and the Local
//! platform's list shows the progress. A second test unplugs the folder that
//! stands for an external drive and checks that its files are shelved, not
//! lost, and come back matched.
//!
//! Nothing here touches the network: the scanner runs without hydration and
//! the catalog cache is seeded with each played media's episode count.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::Duration,
};

use ::library::Candidate;
use db::{Db, LocalFile};
use domain::{AppEvent, ExternalId, FuzzyDate, ListStatus, MediaId, StreamSource};
use futures::{SinkExt, channel::mpsc};
use metadata::{CatalogCache, MediaDetail, MediaSummary, MediaTitle};
use player::PlayerEvent;
use settings::{LibrarySettings, PlaybackSettings};
use tokio::{runtime::Handle, time::timeout};
use tracker::Local;

use crate::{
    library::{
        LibraryScanner,
        fakes::{FakeCandidates, next_finish},
    },
    playback::{LocalProgressSync, PlayRequest, PlaybackManager, fakes::FakeLauncher},
};

const FRIEREN: u32 = 154_587;
const ONE_PIECE: u32 = 21;
const ONE_PIECE_FILM_RED: u32 = 141_902;
const MUSHOKU_TENSEI: u32 = 108_465;
const MUSHOKU_TENSEI_2: u32 = 146_065;
const BOCCHI: u32 = 130_003;
const DUNGEON_MESHI: u32 = 153_518;

/// One episode's length as the fake player reports it, in seconds.
const EPISODE: f64 = 1420.0;

/// Releases on the internal disk: four groups, an absolute-numbered long
/// runner, a season-2 media and NCOP/NCED files in a show folder.
const INTERNAL: &[&str] = &[
    "[SubsPlease] Sousou no Frieren - 01 (1080p) [F02B9CEE].mkv",
    "[SubsPlease] Sousou no Frieren - 02 (1080p) [8D7C1A2B].mkv",
    "[Erai-raws] One Piece - 1085 [1080p][Multiple Subtitle][5A8B3C21].mkv",
    "[Erai-raws] One Piece - 1086 [1080p][Multiple Subtitle][1F2E3D4C].mkv",
    "[SubsPlease] Mushoku Tensei S2 - 01 (1080p) [EC64C8B1].mkv",
    "[SubsPlease] Mushoku Tensei S2 - 02 (1080p) [3B9A0C7D].mkv",
    "Bocchi the Rock!/[Judas] Bocchi the Rock! - 01 [1080p].mkv",
    "Bocchi the Rock!/[Judas] Bocchi the Rock! - NCOP [1080p].mkv",
    "Bocchi the Rock!/[Judas] Bocchi the Rock! - NCED [1080p].mkv",
];

/// Releases on the external drive.
const EXTERNAL: &[&str] = &[
    "[EMBER] Dungeon Meshi - 01 [1080p].mkv",
    "[EMBER] Dungeon Meshi - 02 [1080p].mkv",
];

/// The titles an anime-offline-database index would supply, with the
/// synonyms that tell the seasons apart.
fn candidates() -> Vec<Candidate> {
    vec![
        Candidate::new("Sousou no Frieren", vec![ExternalId::AniList(FRIEREN)])
            .with_alias("Frieren: Beyond Journey's End"),
        Candidate::new(
            "One Piece Film: Red",
            vec![ExternalId::AniList(ONE_PIECE_FILM_RED)],
        ),
        Candidate::new("One Piece", vec![ExternalId::AniList(ONE_PIECE)]),
        Candidate::new(
            "Mushoku Tensei: Isekai Ittara Honki Dasu",
            vec![ExternalId::AniList(MUSHOKU_TENSEI)],
        )
        .with_alias("Mushoku Tensei: Jobless Reincarnation"),
        Candidate::new(
            "Mushoku Tensei II: Isekai Ittara Honki Dasu",
            vec![ExternalId::AniList(MUSHOKU_TENSEI_2)],
        )
        .with_alias("Mushoku Tensei: Jobless Reincarnation Season 2"),
        Candidate::new("Bocchi the Rock!", vec![ExternalId::AniList(BOCCHI)]),
        Candidate::new("Dungeon Meshi", vec![ExternalId::AniList(DUNGEON_MESHI)])
            .with_alias("Delicious in Dungeon"),
    ]
}

/// A library on disk with a scanner and a playback manager over one database.
struct Setup {
    dir: tempfile::TempDir,
    db: Db,
    catalog: CatalogCache,
    scanner: LibraryScanner,
    scans: flume::Receiver<AppEvent>,
    manager: Arc<PlaybackManager>,
    launcher: FakeLauncher,
    playback: flume::Receiver<AppEvent>,
}

impl Setup {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        for (folder, names) in [("internal", INTERNAL), ("external", EXTERNAL)] {
            for name in names {
                let path = dir.path().join(folder).join(name);
                fs::create_dir_all(path.parent().unwrap()).unwrap();
                fs::write(&path, b"").unwrap();
            }
        }
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        let cache = dir.path().join("cache");
        fs::create_dir_all(&cache).unwrap();
        let client = net::Client::new().unwrap();
        let poster = net::PosterCache::new(&cache).unwrap();
        let catalog = CatalogCache::new(metadata::AniList::new(client), poster);

        let settings = LibrarySettings {
            folders: vec![dir.path().join("internal"), dir.path().join("external")],
            fetch_filler_episodes: false,
            ..LibrarySettings::default()
        };
        let (scan_events, scans) = flume::unbounded();
        let scanner = LibraryScanner::new(
            db.clone(),
            Arc::new(FakeCandidates {
                titles: candidates(),
                gate: Arc::new(Mutex::new(None)),
            }),
            Arc::new(RwLock::new(settings)),
            Arc::new(Mutex::new(Some(scan_events))),
            Handle::current(),
        );

        let (playback_events, playback) = flume::unbounded();
        let launcher = FakeLauncher::default();
        let manager = Arc::new(PlaybackManager::with_launcher(
            db.clone(),
            catalog.clone(),
            Arc::new(LocalProgressSync::new(db.clone())),
            playback_events,
            &PlaybackSettings::default(),
            cache,
            Arc::new(launcher.clone()),
        ));
        Self {
            dir,
            db,
            catalog,
            scanner,
            scans,
            manager,
            launcher,
            playback,
        }
    }

    fn external(&self) -> PathBuf {
        self.dir.path().join("external")
    }

    /// Runs one scan as the Scan button does and waits for it to finish.
    async fn scan(&self) {
        self.scanner.start().unwrap();
        let (.., error) = next_finish(&self.scans, Duration::from_secs(5))
            .await
            .expect("the scan finishes");
        assert_eq!(error, None, "the scan succeeded");
    }

    async fn files(&self) -> Vec<LocalFile> {
        self.db.local_file().all().await.unwrap()
    }

    async fn file(&self, name: &str) -> LocalFile {
        self.files()
            .await
            .into_iter()
            .find(|file| file.path.ends_with(name))
            .expect("the file is in the library")
    }

    async fn media(&self, anilist_id: u32) -> MediaId {
        self.db
            .media()
            .find_or_mint(domain::MediaKind::Anime, &[ExternalId::AniList(anilist_id)])
            .await
            .unwrap()
            .media
    }

    /// Caches the media's Entry detail, as opening its Entry page would, so
    /// the watched sync reads the episode count without a request.
    async fn open_entry(&self, anilist_id: u32, episodes: Option<u32>) {
        let detail = detail(anilist_id, episodes);
        self.catalog
            .section(
                format!("media:{anilist_id}"),
                Duration::from_hours(24),
                async move { Ok(detail) },
            )
            .await
            .unwrap();
    }

    /// Plays a library file and returns the fake player's event sender.
    async fn play(&self, file: &LocalFile) -> mpsc::UnboundedSender<PlayerEvent> {
        let request = PlayRequest::new(
            file.media_id.expect("the file is matched"),
            file.episode.expect("the file has an episode number"),
            StreamSource::local(file.path.clone()),
        );
        self.manager.play(request).await.unwrap();
        self.launcher
            .senders
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("the player was launched")
    }

    /// Waits until the manager has handled an event that published `seconds`.
    async fn handled(&self, seconds: f64) {
        let wanted = Duration::from_secs_f64(seconds);
        timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(AppEvent::PlaybackState(state)) = self.playback.recv_async().await
                    && state.position == wanted
                    && state.duration.is_some()
                {
                    return;
                }
            }
        })
        .await
        .expect("the manager published the position");
    }
}

fn detail(anilist_id: u32, episodes: Option<u32>) -> MediaDetail {
    MediaDetail {
        summary: MediaSummary {
            anilist_id,
            mal_id: None,
            kind: Some(domain::MediaKind::Anime),
            title: MediaTitle::default(),
            poster: None,
            banner: None,
            format: None,
            status: None,
            season: None,
            season_year: None,
            episodes,
            chapters: None,
            mean_score: None,
            genres: Vec::new(),
            is_adult: false,
        },
        description: None,
        start_date: FuzzyDate::UNKNOWN,
        end_date: FuzzyDate::UNKNOWN,
        duration: None,
        studios: Vec::new(),
        popularity: None,
        characters: Vec::new(),
        relations: Vec::new(),
        recommendations: Vec::new(),
    }
}

async fn ready(events: &mut mpsc::UnboundedSender<PlayerEvent>) {
    events
        .send(PlayerEvent::MediaReady {
            path: None,
            duration: Some(EPISODE),
        })
        .await
        .unwrap();
}

async fn position(events: &mut mpsc::UnboundedSender<PlayerEvent>, seconds: f64) {
    events
        .send(PlayerEvent::Position { seconds })
        .await
        .unwrap();
}

fn names(files: &[LocalFile], media: MediaId) -> Vec<String> {
    let mut names: Vec<String> = files
        .iter()
        .filter(|file| file.media_id == Some(media))
        .filter_map(|file| file.path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// Scan → matched media: every release lands on its own media, the film
/// and the first season are not picked, and Home lists the five media.
async fn assert_five_media_matched(setup: &Setup) {
    let files = setup.files().await;
    assert_eq!(files.len(), INTERNAL.len() + EXTERNAL.len());
    assert!(
        files.iter().all(|file| file.media_id.is_some()),
        "every release matched: {:?}",
        files
            .iter()
            .filter(|file| file.media_id.is_none())
            .map(|file| &file.path)
            .collect::<Vec<_>>()
    );
    let frieren = setup.media(FRIEREN).await;
    let one_piece = setup.media(ONE_PIECE).await;
    let mushoku_2 = setup.media(MUSHOKU_TENSEI_2).await;
    let bocchi = setup.media(BOCCHI).await;
    let meshi = setup.media(DUNGEON_MESHI).await;
    assert_eq!(names(&files, frieren).len(), 2);
    assert_eq!(names(&files, one_piece).len(), 2, "not the film");
    assert_eq!(
        names(&files, mushoku_2).len(),
        2,
        "the S2 files belong to the second season"
    );
    assert_eq!(names(&files, bocchi).len(), 3);
    assert_eq!(names(&files, meshi).len(), 2);
    assert!(
        names(&files, setup.media(MUSHOKU_TENSEI).await).is_empty(),
        "nothing went to the first season"
    );
    let mut media = crate::library::matched_media(&setup.db).await.unwrap();
    media.sort_by_key(|id| id.get());
    let mut expected = vec![frieren, one_piece, mushoku_2, bocchi, meshi];
    expected.sort_by_key(|id| id.get());
    assert_eq!(media, expected, "Home lists the five media");
}

#[tokio::test]
async fn a_scanned_fansub_folder_plays_resumes_and_syncs_local_progress() {
    let setup = Setup::new().await;
    setup.scan().await;

    assert_five_media_matched(&setup).await;
    let frieren = setup.media(FRIEREN).await;
    let one_piece = setup.media(ONE_PIECE).await;

    // The long runner keeps its absolute numbers; themes are not episodes.
    let long_runner = setup
        .file("[Erai-raws] One Piece - 1085 [1080p][Multiple Subtitle][5A8B3C21].mkv")
        .await;
    assert_eq!(
        (long_runner.kind.as_str(), long_runner.episode),
        ("episode", Some(1085))
    );
    let opening = setup
        .file("[Judas] Bocchi the Rock! - NCOP [1080p].mkv")
        .await;
    let ending = setup
        .file("[Judas] Bocchi the Rock! - NCED [1080p].mkv")
        .await;
    assert_eq!(
        (opening.kind.as_str(), ending.kind.as_str()),
        ("ncop", "nced")
    );
    let season_two = setup
        .file("[SubsPlease] Mushoku Tensei S2 - 02 (1080p) [3B9A0C7D].mkv")
        .await;
    assert_eq!(season_two.episode, Some(2));

    // Play → stop midway: the position is kept and nothing is synced.
    setup.open_entry(FRIEREN, Some(28)).await;
    let episode = setup
        .file("[SubsPlease] Sousou no Frieren - 01 (1080p) [F02B9CEE].mkv")
        .await;
    let mut player = setup.play(&episode).await;
    ready(&mut player).await;
    position(&mut player, 600.0).await;
    setup.handled(600.0).await;
    setup.manager.stop().await;
    assert!(
        setup.launcher.seeks().is_empty(),
        "a first play starts at 0"
    );
    let saved = setup.db.continuity().get(frieren, 1).await.unwrap();
    assert_eq!(saved.map(|point| point.position), Some(600.0));
    let local = Local::new(setup.db.clone());
    assert_eq!(local.entry(frieren).await.unwrap(), None, "not watched yet");

    // Replay → the seek waits for media-ready.
    let mut player = setup.play(&episode).await;
    position(&mut player, 0.0).await;
    tokio::task::yield_now().await;
    assert!(
        setup.launcher.seeks().is_empty(),
        "no seek before media-ready"
    );
    ready(&mut player).await;
    setup.handled(0.0).await;
    assert_eq!(setup.launcher.seeks(), [600.0], "resumed where it stopped");

    // Watched past the 80% threshold → Local progress.
    position(&mut player, 1200.0).await;
    setup.handled(1200.0).await;
    setup.manager.stop().await;
    let entry = local.entry(frieren).await.unwrap().expect("a Local entry");
    assert_eq!((entry.status, entry.progress), (ListStatus::Current, 1));
    assert_eq!(
        setup.db.continuity().get(frieren, 1).await.unwrap(),
        None,
        "a watched episode has no resume point"
    );

    // The long runner syncs its absolute number.
    setup.open_entry(ONE_PIECE, None).await;
    let mut player = setup.play(&long_runner).await;
    ready(&mut player).await;
    position(&mut player, 1300.0).await;
    setup.handled(1300.0).await;
    setup.manager.stop().await;
    let entry = local
        .entry(one_piece)
        .await
        .unwrap()
        .expect("a Local entry");
    assert_eq!((entry.status, entry.progress), (ListStatus::Current, 1085));
}

#[tokio::test]
async fn an_unplugged_drive_shelves_its_files_and_they_return_matched() {
    let setup = Setup::new().await;
    setup.scan().await;
    let meshi = setup.media(DUNGEON_MESHI).await;
    let before = setup.files().await;
    assert_eq!(names(&before, meshi).len(), 2);

    let unplugged = setup.dir.path().join("unplugged");
    fs::rename(setup.external(), &unplugged).unwrap();
    setup.scan().await;
    let summary = setup.db.scan_summary().latest().await.unwrap().unwrap();
    assert_eq!(summary.shelved, EXTERNAL.len());
    let during = setup.files().await;
    assert_eq!(during.len(), before.len(), "no row was deleted");
    for name in EXTERNAL {
        let file = setup.file(name).await;
        assert!(file.shelved, "{name} is shelved");
        assert_eq!(file.media_id, Some(meshi), "{name} keeps its match");
    }
    assert!(
        during
            .iter()
            .filter(|file| !is_on(file, &setup.external()))
            .all(|file| !file.shelved && file.media_id.is_some()),
        "the internal disk is untouched"
    );

    fs::rename(&unplugged, setup.external()).unwrap();
    setup.scan().await;
    let after = setup.files().await;
    assert_eq!(after.len(), before.len());
    for name in EXTERNAL {
        let file = setup.file(name).await;
        assert!(!file.shelved, "{name} is back");
        assert_eq!(file.media_id, Some(meshi), "{name} is matched again");
        assert_eq!(file.episode, before_episode(&before, name));
    }
}

fn is_on(file: &LocalFile, folder: &Path) -> bool {
    file.path.starts_with(folder)
}

fn before_episode(files: &[LocalFile], name: &str) -> Option<u32> {
    files
        .iter()
        .find(|file| file.path.ends_with(name))
        .and_then(|file| file.episode)
}
