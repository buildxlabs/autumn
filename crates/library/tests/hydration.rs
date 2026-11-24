//! Hydration tracer bullets: absolute numbers, specials and filler.

use std::{fs, slice};

use domain::{ExternalId, MediaKind};
use library::Candidate;
use support::{Filler, Mock};

#[tokio::test]
async fn hydration_numbers_absolute_episodes_and_splits_special_kinds() {
    let mock = Mock::new(Filler::Enabled).await;
    let folder = mock.dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    for name in [
        "Test Series S2 - 03.mkv",
        "Test Series - 28.mkv",
        "Test Series NCOP1.mkv",
        "Test Series NCED.mkv",
        "Test Series OVA.mkv",
    ] {
        fs::write(folder.join(name), b"").unwrap();
    }

    let candidates = [Candidate::new(
        "Test Series",
        vec![ExternalId::AniList(1001)],
    )];
    mock.library
        .scan(slice::from_ref(&folder), &candidates, |_| {})
        .await
        .unwrap();

    let rows = mock.db.local_file().all().await.unwrap();
    let find = |name: &str| {
        rows.iter()
            .find(|row| row.path.file_name().is_some_and(|file| file == name))
            .unwrap()
    };
    let s2 = find("Test Series S2 - 03.mkv");
    assert_eq!((s2.season, s2.episode), (Some(2), Some(3)));
    assert_eq!(s2.kind, "episode");

    let absolute = find("Test Series - 28.mkv");
    assert_eq!(
        (absolute.season, absolute.episode),
        (Some(2), Some(4)),
        "absolute 28 is season two episode four"
    );
    assert!(absolute.filler, "the recorded filler list marks 28");

    assert_eq!(find("Test Series NCOP1.mkv").kind, "ncop");
    assert_eq!(find("Test Series NCED.mkv").kind, "nced");
    assert_eq!(find("Test Series OVA.mkv").kind, "special");

    let requests = mock.requests();
    assert_eq!(
        requests
            .iter()
            .filter(|path| *path == "/v1/episodes")
            .count(),
        1,
        "one AniZip lookup for the media"
    );
    assert_eq!(
        requests.iter().filter(|path| *path == "/shows").count(),
        1,
        "one filler lookup for the media"
    );
}

#[tokio::test]
async fn an_out_of_range_number_is_retried_on_the_sequel() {
    let mock = Mock::new(Filler::Enabled).await;
    let folder = mock.dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("Test Series - 99.mkv"), b"").unwrap();

    let candidates = [
        Candidate::new("Test Series", vec![ExternalId::AniList(1001)]).with_sequel(2002),
        Candidate::new("Test Series 2", vec![ExternalId::AniList(2002)]),
    ];
    mock.library
        .scan(slice::from_ref(&folder), &candidates, |_| {})
        .await
        .unwrap();

    let rows = mock.db.local_file().all().await.unwrap();
    let row = rows.first().unwrap();
    let sequel = mock
        .db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(2002)])
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.media_id, Some(sequel), "the file moved to the sequel");
    assert_eq!((row.season, row.episode), (Some(1), Some(5)));
    assert_eq!(
        mock.requests()
            .iter()
            .filter(|path| *path == "/v1/episodes")
            .count(),
        2,
        "the media and its sequel were each looked up once"
    );
}

#[tokio::test]
async fn a_locked_file_keeps_its_hydrated_number_on_a_rescan() {
    let mock = Mock::new(Filler::Enabled).await;
    let folder = mock.dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    let path = folder.join("Test Series - 28.mkv");
    fs::write(&path, b"").unwrap();
    let candidates = [Candidate::new(
        "Test Series",
        vec![ExternalId::AniList(1001)],
    )];

    mock.library
        .scan(slice::from_ref(&folder), &candidates, |_| {})
        .await
        .unwrap();
    mock.library.lock_file(&path).await.unwrap();
    mock.library
        .scan(slice::from_ref(&folder), &candidates, |_| {})
        .await
        .unwrap();

    let rows = mock.db.local_file().all().await.unwrap();
    let row = rows.first().unwrap();
    assert_eq!((row.season, row.episode), (Some(2), Some(4)));
    assert!(row.locked);
}

#[tokio::test]
async fn disabling_the_filler_lookup_leaves_episodes_unmarked() {
    let mock = Mock::new(Filler::Disabled).await;
    let folder = mock.dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("Test Series - 28.mkv"), b"").unwrap();
    let candidates = [Candidate::new(
        "Test Series",
        vec![ExternalId::AniList(1001)],
    )];

    mock.library
        .scan(slice::from_ref(&folder), &candidates, |_| {})
        .await
        .unwrap();

    let rows = mock.db.local_file().all().await.unwrap();
    assert!(!rows.first().unwrap().filler);
    assert!(
        !mock.requests().iter().any(|path| path == "/shows"),
        "the disabled lookup made no request"
    );
}

/// Loopback metadata clients and a temporary database, in `cfg(test)` so the
/// workspace's test-only lint allowances apply.
#[cfg(test)]
mod support {
    use std::{
        collections::HashMap,
        sync::{Arc, Mutex},
    };

    use axum::{Router, extract::Query, http::StatusCode, routing::get};
    use db::Db;
    use library::Library;
    use metadata::{AniZip, AnimeFillerList, FillerLookup};
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    const MAIN: &str = include_str!("fixtures/hydration_anizip_main.json");
    const SEQUEL: &str = include_str!("fixtures/hydration_anizip_sequel.json");
    const SHOWS: &str = include_str!("fixtures/hydration_filler_shows.html");
    const SHOW: &str = include_str!("fixtures/hydration_filler_show.html");

    #[derive(Debug, Clone, Copy)]
    pub(crate) enum Filler {
        Enabled,
        Disabled,
    }

    pub(crate) struct Mock {
        pub(crate) library: Library,
        pub(crate) db: Db,
        pub(crate) dir: TempDir,
        requests: Arc<Mutex<Vec<String>>>,
    }

    impl Mock {
        pub(crate) async fn new(filler: Filler) -> Self {
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen_episodes = Arc::clone(&requests);
            let seen_shows = Arc::clone(&requests);
            let seen_show = Arc::clone(&requests);
            let app = Router::new()
                .route(
                    "/v1/episodes",
                    get(move |Query(params): Query<HashMap<String, String>>| {
                        let seen = Arc::clone(&seen_episodes);
                        async move {
                            seen.lock().unwrap().push("/v1/episodes".to_owned());
                            let id = params.get("anilist_id").map(String::as_str);
                            let body = match id {
                                Some("1001") => MAIN,
                                Some("2002") => SEQUEL,
                                _ => "{}",
                            };
                            (StatusCode::OK, body.to_owned())
                        }
                    }),
                )
                .route(
                    "/shows",
                    get(move || {
                        let seen = Arc::clone(&seen_shows);
                        async move {
                            seen.lock().unwrap().push("/shows".to_owned());
                            (StatusCode::OK, SHOWS.to_owned())
                        }
                    }),
                )
                .route(
                    "/shows/test-series",
                    get(move || {
                        let seen = Arc::clone(&seen_show);
                        async move {
                            seen.lock().unwrap().push("/shows/test-series".to_owned());
                            (StatusCode::OK, SHOW.to_owned())
                        }
                    }),
                );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });

            let dir = TempDir::new().unwrap();
            let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
            let endpoint = format!("http://{address}");
            let anizip = AniZip::with_endpoint(
                net::Client::builder().build().unwrap(),
                net::PosterCache::new(dir.path()).unwrap(),
                endpoint.clone(),
            );
            let lookup = match filler {
                Filler::Enabled => FillerLookup::Enabled,
                Filler::Disabled => FillerLookup::Disabled,
            };
            let filler = AnimeFillerList::with_endpoint(
                net::Client::builder().build().unwrap(),
                net::PosterCache::new(dir.path()).unwrap(),
                endpoint,
                lookup,
            );
            Self {
                library: Library::with_metadata(db.clone(), anizip, filler),
                db,
                dir,
                requests,
            }
        }

        pub(crate) fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }
}
