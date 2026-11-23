//! The AniList platform and the active-platform switch against a loopback
//! mock and a real SQLite database.

#![expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test harness: fixtures and JSON assertions index recorded data"
)]

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use db::Db;
use domain::{
    AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tracker::{ActiveKind, ActivePlatform, AniListPlatform, ListName, Local, Platform};

struct Mock {
    client: metadata::AniList,
    requests: Arc<Mutex<Vec<Value>>>,
}

impl Mock {
    async fn serve() -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(seen): State<Arc<Mutex<Vec<Value>>>>,
                          Json(request): Json<Value>| {
                        let query = request["query"].as_str().unwrap_or_default().to_owned();
                        async move {
                            seen.lock().unwrap().push(request);
                            let body = if query.contains("MediaListCollection(") {
                                fixture()
                            } else if query.contains("SaveMediaListEntry(") {
                                json!({"data": {"SaveMediaListEntry": {"id": 9001}}})
                            } else if query.contains("DeleteMediaListEntry(") {
                                json!({"data": {"DeleteMediaListEntry": {"deleted": true}}})
                            } else {
                                json!({"data": {}})
                            };
                            (StatusCode::OK, Json(body))
                        }
                    },
                ),
            )
            .with_state(Arc::clone(&requests));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = net::Client::builder()
            .host("127.0.0.1", 6000, 100, net::HostKind::AniList)
            .max_retries(0)
            .build()
            .unwrap();
        Self {
            client: metadata::AniList::with_endpoint(http, format!("http://{address}/")),
            requests,
        }
    }

    fn request(&self, at: usize) -> Value {
        self.requests.lock().unwrap()[at].clone()
    }

    fn count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

fn fixture() -> Value {
    let path = format!(
        "{}/../metadata/tests/fixtures/collection_with_custom_lists.json",
        env!("CARGO_MANIFEST_DIR")
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn entry(media: MediaId, status: ListStatus, progress: u32) -> ListEntry {
    ListEntry {
        media,
        status,
        score: Score::UNSCORED,
        progress,
        progress_volumes: 0,
        repeat: 0,
        started_at: FuzzyDate::UNKNOWN,
        completed_at: FuzzyDate::UNKNOWN,
        notes: String::new(),
        private: false,
        priority: 0,
        advanced_scores: AdvancedScores::default(),
        custom_lists: Vec::new(),
        hidden_from_status_lists: false,
        created_at: std::time::SystemTime::UNIX_EPOCH,
        updated_at: std::time::SystemTime::UNIX_EPOCH,
    }
}

async fn db_at(path: &std::path::Path) -> Db {
    Db::open(path).await.unwrap()
}

fn platform(db: Db, mock: &Mock) -> AniListPlatform {
    AniListPlatform::new(
        db,
        mock.client.clone(),
        "test-token".to_owned(),
        "TestUser".to_owned(),
    )
}

#[tokio::test]
async fn the_collection_groups_status_and_custom_lists() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let mock = Mock::serve().await;
    let anilist = platform(db.clone(), &mock);

    let collection = anilist.collection(MediaKind::Anime).await.unwrap();
    let names: Vec<&ListName> = collection.lists.iter().map(|list| &list.name).collect();
    assert_eq!(
        names,
        vec![
            &ListName::Status(ListStatus::Current),
            &ListName::Custom("Favourites".to_owned()),
        ],
        "custom lists included"
    );

    let watching = &collection.lists[0].entries[0];
    assert_eq!(watching.status, ListStatus::Current);
    assert_eq!(watching.progress, 12);
    assert_eq!(watching.score.raw(), 85);
    assert_eq!(watching.custom_lists, vec!["Favourites"]);

    let hidden = &collection.lists[1].entries[1];
    assert!(hidden.hidden_from_status_lists);
    assert_eq!(hidden.progress, 24);

    let media = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(154_587)])
        .await
        .unwrap()
        .expect("the AniList id was minted locally");
    assert_eq!(media, watching.media, "one media per AniList id");
}

#[tokio::test]
async fn save_and_delete_use_the_cached_list_entry_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let mock = Mock::serve().await;
    let anilist = platform(db.clone(), &mock);
    anilist.collection(MediaKind::Anime).await.unwrap();
    assert!(!anilist.collection_truncated(), "one chunk in the fixture");

    let media = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(154_587)])
        .await
        .unwrap()
        .expect("the fixture media was minted");
    let mut edited = entry(media, ListStatus::Current, 13);
    edited.score = Score::new(85).unwrap();
    anilist.save_entry(&edited).await.unwrap();
    let variables = mock.request(1)["variables"].clone();
    assert_eq!(
        variables["id"], 9001,
        "the list-entry id came from the collection"
    );
    assert_eq!(variables["scoreRaw"], 85);
    assert_eq!(variables["progress"], 13);
    assert_eq!(variables["status"], "CURRENT");

    anilist.delete_entry(media).await.unwrap();
    assert_eq!(mock.request(2)["variables"]["id"], 9001);
    assert_eq!(mock.count(), 3, "delete needed no lookup request");
}

#[tokio::test]
async fn deleting_on_a_cold_cache_looks_up_the_entry_id() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
        .await
        .unwrap();
    let mock = Mock::serve().await;
    let anilist = platform(db, &mock);

    anilist.delete_entry(minted.media).await.unwrap();
    let last = mock.request(mock.count() - 1);
    assert_eq!(last["variables"]["id"], 9001, "id came from a lookup");
    assert!(
        !anilist.collection_truncated(),
        "the fixture fits one chunk"
    );
}

#[tokio::test]
async fn connecting_anilist_keeps_local_entries_dormant_and_sign_out_restores_them() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("autumn.db");
    let db = db_at(&db_path).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
        .await
        .unwrap();
    db.local_list()
        .save(&entry(minted.media, ListStatus::Current, 5))
        .await
        .unwrap();

    let mock = Mock::serve().await;
    let mut active = ActivePlatform::new(Local::new(db.clone()));
    assert_eq!(active.active(), ActiveKind::Local);
    let local_collection = active.collection(MediaKind::Anime).await.unwrap();
    assert_eq!(local_collection.lists[0].entries[0].progress, 5);

    active.connect(platform(db.clone(), &mock));
    assert_eq!(active.active(), ActiveKind::AniList);
    let remote = active.collection(MediaKind::Anime).await.unwrap();
    assert_eq!(remote.lists[0].entries[0].progress, 12, "AniList is active");
    let dormant = db.local_list().entry(minted.media).await.unwrap().unwrap();
    assert_eq!(dormant.progress, 5, "Local was not written while dormant");

    active.sign_out();
    assert_eq!(active.active(), ActiveKind::Local, "Local comes back");
    let restored = active.collection(MediaKind::Anime).await.unwrap();
    assert_eq!(restored.lists[0].entries[0].progress, 5, "untouched");
}
