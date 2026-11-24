//! The AniList platform and the active-platform switch against a loopback
//! mock and a real SQLite database.

#![expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test harness: fixtures and JSON assertions index recorded data"
)]

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use db::Db;
use domain::{
    AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;
use tracker::{ActiveKind, ActivePlatform, AniListPlatform, ListName, Local, Platform};

/// Recorded requests, the collection body, the 401 flags, and fields
/// AniList changes on its own when it saves an entry.
type MockState = (
    Arc<Mutex<Vec<Value>>>,
    Arc<AtomicBool>,
    Arc<Mutex<Value>>,
    Arc<AtomicBool>,
    Arc<Mutex<Value>>,
);

struct Mock {
    client: metadata::AniList,
    requests: Arc<Mutex<Vec<Value>>>,
    reject_token: Arc<AtomicBool>,
    bare_401: Arc<AtomicBool>,
    server_changes: Arc<Mutex<Value>>,
}

/// What AniList answers to `SaveMediaListEntry`: the stored entry (from the
/// collection when it exists, else a new one) with the sent fields applied,
/// then any `server_changes`.
fn saved_entry(collection: &Value, variables: &Value, server_changes: &Value) -> Value {
    let existing = collection["data"]["MediaListCollection"]["lists"]
        .as_array()
        .into_iter()
        .flatten()
        .flat_map(|group| group["entries"].as_array().cloned().unwrap_or_default())
        .find(|entry| {
            entry["id"] == variables["id"]
                || (variables["id"].is_null() && entry["mediaId"] == variables["mediaId"])
        });
    let mut entry = existing.unwrap_or_else(|| {
        // A new entry, with every field AniList selects present.
        json!({
            "id": 9100, "mediaId": variables["mediaId"], "status": "CURRENT",
            "score": 0, "progress": 0, "progressVolumes": 0, "repeat": 0,
            "priority": 0, "private": false, "notes": null,
            "hiddenFromStatusLists": false, "startedAt": null, "completedAt": null,
        })
    });
    for (sent, field) in [
        ("status", "status"),
        ("scoreRaw", "score"),
        ("progress", "progress"),
        ("progressVolumes", "progressVolumes"),
        ("repeat", "repeat"),
        ("priority", "priority"),
        ("private", "private"),
        ("notes", "notes"),
        ("hidden", "hiddenFromStatusLists"),
        ("startedAt", "startedAt"),
        ("completedAt", "completedAt"),
    ] {
        if let Some(value) = variables.get(sent).filter(|value| !value.is_null()) {
            entry[field] = value.clone();
        }
    }
    let enabled: Vec<Value> = variables["customLists"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    entry["customLists"] = Value::Array(
        enabled
            .into_iter()
            .map(|name| json!({"name": name, "enabled": true}))
            .collect(),
    );
    if let Some(changes) = server_changes.as_object() {
        for (field, value) in changes {
            entry[field] = value.clone();
        }
    }
    entry
}

impl Mock {
    async fn serve() -> Self {
        Self::serve_with(fixture()).await
    }

    async fn serve_with(body: Value) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let body = Arc::new(Mutex::new(body));
        let reject_token = Arc::new(AtomicBool::new(false));
        let bare_401 = Arc::new(AtomicBool::new(false));
        let server_changes = Arc::new(Mutex::new(Value::Null));
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State((seen, reject_token, collection_body, bare_401, changes)): State<MockState>,
                          Json(request): Json<Value>| {
                        let query = request["query"].as_str().unwrap_or_default().to_owned();
                        let variables = request["variables"].clone();
                        let reject = reject_token.load(Ordering::SeqCst);
                        let bare = bare_401.load(Ordering::SeqCst);
                        async move {
                            seen.lock().unwrap().push(request);
                            if bare {
                                return (StatusCode::UNAUTHORIZED, Json(json!({})));
                            }
                            let body = if reject {
                                json!({"errors": [{"message": "Unauthorized.", "status": 401}], "data": null})
                            } else if query.contains("MediaListCollection(") {
                                collection_body.lock().unwrap().clone()
                            } else if query.contains("SaveMediaListEntry(") {
                                let collection = collection_body.lock().unwrap().clone();
                                let changes = changes.lock().unwrap().clone();
                                json!({"data": {"SaveMediaListEntry":
                                    saved_entry(&collection, &variables, &changes)}})
                            } else if query.contains("mediaListOptions") {
                                let collection = collection_body.lock().unwrap().clone();
                                json!({"data": {"Viewer": collection["data"]["Viewer"]}})
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
            .with_state((
                Arc::clone(&requests),
                Arc::clone(&reject_token),
                Arc::clone(&body),
                Arc::clone(&bare_401),
                Arc::clone(&server_changes),
            ));
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
            reject_token,
            bare_401,
            server_changes,
        }
    }

    /// Fields AniList sets on its own in the next saved entries.
    fn change_on_save(&self, changes: Value) {
        *self.server_changes.lock().unwrap() = changes;
    }

    fn reject_token(&self) {
        self.reject_token.store(true, Ordering::SeqCst);
    }

    fn reject_http_401(&self) {
        self.bare_401.store(true, Ordering::SeqCst);
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

fn fixture_with_empty_custom_list() -> Value {
    let path = format!(
        "{}/../metadata/tests/fixtures/collection_with_empty_custom_list.json",
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
async fn a_bare_http_401_is_also_classified_as_unauthorized() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
        .await
        .unwrap();
    let mock = Mock::serve().await;
    let anilist = platform(db, &mock);
    mock.reject_http_401();

    let error = anilist
        .save_entry(&entry(minted.media, ListStatus::Current, 3))
        .await
        .unwrap_err();

    assert!(
        matches!(error, tracker::PlatformError::Unauthorized),
        "a bare HTTP 401 reaches the UI as Unauthorized: {error:?}"
    );
}

#[tokio::test]
async fn update_progress_on_a_cold_cache_keeps_an_existing_completed_status() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(52_991)])
        .await
        .unwrap();
    let mock = Mock::serve().await;
    let anilist = platform(db, &mock);

    // No collection() first: the entry must be looked up from AniList.
    let updated = anilist.update_progress(minted.media, 40).await.unwrap();

    let save = mock
        .requests
        .lock()
        .unwrap()
        .iter()
        .find(|request| {
            request["query"]
                .as_str()
                .unwrap_or_default()
                .contains("SaveMediaListEntry(")
        })
        .expect("a save was sent")
        .clone();
    let variables = save["variables"].clone();
    assert!(
        variables
            .get("status")
            .is_none_or(serde_json::Value::is_null),
        "an existing COMPLETED entry is not overwritten: {variables}"
    );
    assert_eq!(variables["id"], 9002, "the looked-up list-entry id is used");
    assert_eq!(variables["progress"], 40);
    assert_eq!(updated.status, ListStatus::Completed, "status survives");
    assert_eq!(updated.progress, 40);
    assert_eq!(updated.media, minted.media);
}

#[tokio::test]
async fn a_401_on_a_write_is_classified_as_unauthorized_not_a_generic_remote_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
        .await
        .unwrap();
    let mock = Mock::serve().await;
    let anilist = platform(db, &mock);
    mock.reject_token();

    let error = anilist
        .save_entry(&entry(minted.media, ListStatus::Current, 3))
        .await
        .unwrap_err();

    assert!(
        matches!(error, tracker::PlatformError::Unauthorized),
        "a 401 reaches the UI as Unauthorized: {error:?}"
    );
}

#[tokio::test]
async fn empty_custom_lists_keep_their_names_for_a_true_union() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let mock = Mock::serve_with(fixture_with_empty_custom_list()).await;
    let anilist = platform(db, &mock);

    let collection = anilist.collection(MediaKind::Anime).await.unwrap();
    let empty = collection
        .lists
        .iter()
        .find(|list| list.name == ListName::Custom("Empty Shelf".to_owned()))
        .expect("an empty custom list is still reported");
    assert!(empty.entries.is_empty(), "and it has no entries");

    let before = mock.count();
    let names = anilist.custom_list_names().await.unwrap();
    assert_eq!(
        names.for_kind(MediaKind::Anime),
        ["Favourites".to_owned(), "Empty Shelf".to_owned()]
    );
    assert_eq!(names.for_kind(MediaKind::Manga), ["Read Later".to_owned()]);
    assert_eq!(mock.count(), before + 1, "names need one request");
    assert!(
        !mock.request(before)["query"]
            .as_str()
            .unwrap()
            .contains("MediaListCollection"),
        "reading names does not download the collection"
    );
}

#[tokio::test]
async fn update_progress_preserves_an_existing_status_and_returns_the_saved_entry() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let mock = Mock::serve().await;
    let anilist = platform(db.clone(), &mock);
    anilist.collection(MediaKind::Anime).await.unwrap();

    let media = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(52_991)])
        .await
        .unwrap()
        .expect("the completed entry was minted by collection");
    let updated = anilist.update_progress(media, 50).await.unwrap();

    let variables = mock.request(mock.count() - 1)["variables"].clone();
    assert!(
        variables
            .get("status")
            .is_none_or(serde_json::Value::is_null),
        "an existing entry's status is not overwritten: {variables}"
    );
    assert_eq!(variables["id"], 9002, "the cached list-entry id is used");
    assert_eq!(variables["progress"], 50);
    assert_eq!(updated.media, media);
    assert_eq!(
        updated.status,
        ListStatus::Completed,
        "the saved entry keeps its status"
    );
    assert_eq!(updated.progress, 50);
    assert!(
        updated.hidden_from_status_lists,
        "unchanged stored fields come back too"
    );
}

#[tokio::test]
async fn update_progress_on_a_new_entry_sets_current_status_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(999_999)])
        .await
        .unwrap();
    let mock = Mock::serve().await;
    let anilist = platform(db, &mock);

    let updated = anilist.update_progress(minted.media, 7).await.unwrap();

    let variables = mock.request(mock.count() - 1)["variables"].clone();
    assert_eq!(
        variables["status"], "CURRENT",
        "a new entry starts in the Current status"
    );
    assert!(
        variables.get("id").is_none_or(serde_json::Value::is_null),
        "a new entry has no list-entry id yet"
    );
    assert_eq!(variables["progress"], 7);
    assert_eq!(updated.status, ListStatus::Current);
    assert_eq!(updated.progress, 7);
    assert_eq!(updated.score, Score::UNSCORED);
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

#[tokio::test]
async fn writes_return_the_entry_anilist_stored_not_the_request() {
    let dir = tempfile::tempdir().unwrap();
    let db = db_at(&dir.path().join("autumn.db")).await;
    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(999_999)])
        .await
        .unwrap();
    let mock = Mock::serve().await;
    let anilist = platform(db, &mock);
    // AniList completes an entry on its last episode and dates it.
    mock.change_on_save(json!({
        "status": "COMPLETED",
        "completedAt": {"year": 2026, "month": 11, "day": 24},
    }));

    let updated = anilist.update_progress(minted.media, 12).await.unwrap();

    assert_eq!(
        updated.status,
        ListStatus::Completed,
        "AniList's status wins"
    );
    assert_eq!(
        updated.completed_at,
        FuzzyDate::new(Some(2026), Some(11), Some(24)).unwrap()
    );
    assert_eq!(updated.progress, 12);

    let mut edited = updated.clone();
    edited.custom_lists = vec!["Favourites".to_owned()];
    edited.notes = "rewatch soon".to_owned();
    mock.change_on_save(json!({"score": 90}));
    let saved = anilist.save_entry(&edited).await.unwrap();
    assert_eq!(saved.score.raw(), 90, "a server-side change comes back");
    assert_eq!(saved.notes, "rewatch soon");
    assert_eq!(
        saved.custom_lists,
        vec!["Favourites"],
        "enabled custom lists are decoded from the saved entry"
    );
    let variables = mock.request(mock.count() - 1)["variables"].clone();
    assert_eq!(variables["id"], 9100, "the new entry's id was cached");
}
