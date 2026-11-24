//! Authenticated collection reads and list mutations against a loopback mock.
//!
//! The fixture is recorded AniList shape: a status group, a custom-list group
//! that repeats one entry, and an entry hidden from status lists that lives
//! only in the custom group.

#![expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    reason = "test harness: fixtures and JSON assertions index recorded data"
)]

use std::sync::{Arc, Mutex};

use axum::{
    Json, Router,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
};
use domain::{FuzzyDate, ListStatus, MediaKind};
use metadata::{AniList, ListEntryInput, ScoreFormat};
use serde_json::{Value, json};
use tokio::net::TcpListener;

#[derive(Debug, Default)]
struct Recorded {
    requests: Vec<Value>,
    auth: Vec<String>,
}

struct Mock {
    client: AniList,
    recorded: Arc<Mutex<Recorded>>,
}

impl Mock {
    async fn serve() -> Self {
        Self::serve_with_status(StatusCode::OK).await
    }

    async fn serve_with_status(status: StatusCode) -> Self {
        let recorded = Arc::new(Mutex::new(Recorded::default()));
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(seen): State<Arc<Mutex<Recorded>>>,
                          headers: HeaderMap,
                          Json(request): Json<Value>| {
                        let status = status;
                        async move {
                            let mut seen = seen.lock().unwrap();
                            seen.requests.push(request.clone());
                            if let Some(auth) = headers.get("authorization") {
                                seen.auth.push(auth.to_str().unwrap().to_owned());
                            }
                            let query = request["query"].as_str().unwrap_or_default();
                            let body = if query.contains("MediaListCollection(") {
                                fixture("collection_with_custom_lists.json")
                            } else if query.contains("SaveMediaListEntry(") {
                                // AniList's answer: the stored entry, with a
                                // status it changed on its own.
                                json!({"data": {"SaveMediaListEntry": {
                                    "id": 9001, "mediaId": 154_587, "status": "COMPLETED",
                                    "score": 85, "progress": 13, "progressVolumes": null,
                                    "repeat": null, "priority": null, "private": true,
                                    "notes": null, "hiddenFromStatusLists": false,
                                    "startedAt": null, "completedAt": null,
                                    "customLists": [
                                        {"name": "Favourites", "enabled": true},
                                        {"name": "Watchlist", "enabled": false}
                                    ]
                                }}})
                            } else if query.contains("mediaListOptions") {
                                json!({"data": {"Viewer": fixture("collection_with_empty_custom_list.json")["data"]["Viewer"]}})
                            } else if query.contains("DeleteMediaListEntry(") {
                                json!({"data": {"DeleteMediaListEntry": {"deleted": true}}})
                            } else if query.contains("UpdateUser(") {
                                json!({"data": {"UpdateUser": {"id": 1}}})
                            } else {
                                json!({"data": {}})
                            };
                            (status, Json(body))
                        }
                    },
                ),
            )
            .with_state(Arc::clone(&recorded));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let http = net::Client::builder()
            .host("127.0.0.1", 6000, 100, net::HostKind::AniList)
            .max_retries(3)
            .build()
            .unwrap();
        Self {
            client: AniList::with_endpoint(http, format!("http://{address}/")),
            recorded,
        }
    }

    fn request(&self, at: usize) -> Value {
        self.recorded.lock().unwrap().requests[at].clone()
    }

    fn count(&self) -> usize {
        self.recorded.lock().unwrap().requests.len()
    }

    fn auth(&self) -> Vec<String> {
        self.recorded.lock().unwrap().auth.clone()
    }
}

fn fixture(name: &str) -> Value {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[tokio::test]
async fn the_collection_decodes_custom_lists_and_hidden_entries() {
    let mock = Mock::serve().await;
    let collection = mock
        .client
        .list_collection("test-token", "TestUser", MediaKind::Anime)
        .await
        .unwrap();

    assert_eq!(collection.score_format, ScoreFormat::Point10Decimal);
    assert!(!collection.has_next_chunk, "one chunk in the fixture");
    assert_eq!(collection.lists.len(), 2, "status and custom groups");
    assert!(!collection.lists[0].custom);
    assert_eq!(collection.lists[0].name, "Watching");
    assert!(collection.lists[1].custom);
    assert_eq!(collection.lists[1].name, "Favourites");

    let visible = &collection.lists[0].entries[0];
    assert_eq!(visible.id, 9001, "the list-entry id is kept for delete");
    assert_eq!(visible.media_id, 154_587);
    assert_eq!(visible.status, ListStatus::Current);
    assert_eq!(visible.score_raw, 85, "score(format: POINT_100) is raw");
    assert_eq!(visible.progress, 12);
    assert_eq!(visible.custom_lists, vec!["Favourites"]);
    assert_eq!(
        visible.started_at,
        FuzzyDate::new(Some(2023), Some(10), Some(5)).unwrap()
    );

    let hidden = &collection.lists[1].entries[1];
    assert_eq!(hidden.media_id, 52991);
    assert!(hidden.hidden_from_status_lists);
    assert_eq!(hidden.score_raw, 100);
    assert_eq!(hidden.custom_lists, vec!["Favourites"]);

    assert_eq!(mock.auth(), vec!["Bearer test-token"]);
}

#[tokio::test]
async fn a_save_sends_every_field_as_raw_score_once() {
    let mock = Mock::serve().await;
    let input = ListEntryInput {
        id: Some(9001),
        status: Some(ListStatus::Current),
        score_raw: Some(85),
        progress: Some(13),
        custom_lists: Some(vec!["Favourites".to_owned()]),
        private: Some(true),
        ..ListEntryInput::default()
    };
    let saved = mock
        .client
        .save_list_entry("test-token", &input)
        .await
        .unwrap();

    assert_eq!(saved.id, 9001, "the list-entry id comes back for deletes");
    assert_eq!(saved.media_id, 154_587);
    assert_eq!(
        saved.status,
        ListStatus::Completed,
        "the stored entry, not the request, comes back"
    );
    assert_eq!(saved.score_raw, 85);
    assert_eq!(saved.progress, 13);
    assert_eq!(
        saved.custom_lists,
        vec!["Favourites"],
        "only enabled custom lists"
    );
    assert!(
        mock.request(0)["query"]
            .as_str()
            .unwrap()
            .contains("customLists(asArray: true)"),
        "the saved entry's custom lists are selected"
    );
    assert_eq!(mock.count(), 1, "a mutation is sent once");
    let variables = mock.request(0)["variables"].clone();
    assert_eq!(variables["id"], 9001);
    assert_eq!(variables["status"], "CURRENT");
    assert_eq!(
        variables["scoreRaw"], 85,
        "raw score, not the account format"
    );
    assert_eq!(variables["progress"], 13);
    assert_eq!(variables["customLists"][0], "Favourites");
    assert_eq!(variables["private"], true);
    assert!(
        variables.get("mediaId").is_none(),
        "unset fields are omitted"
    );
    assert_eq!(mock.auth(), vec!["Bearer test-token"]);
}

#[tokio::test]
async fn delete_and_custom_list_creation_use_their_mutations() {
    let mock = Mock::serve().await;
    mock.client
        .delete_list_entry("test-token", 9001)
        .await
        .unwrap();
    mock.client
        .update_custom_lists(
            "test-token",
            MediaKind::Anime,
            &["Favourites".to_owned(), "Watchlist".to_owned()],
        )
        .await
        .unwrap();

    assert_eq!(mock.count(), 2);
    assert_eq!(mock.request(0)["variables"]["id"], 9001);
    let variables = mock.request(1)["variables"].clone();
    assert_eq!(variables["anime"]["customLists"][0], "Favourites");
    assert_eq!(variables["anime"]["customLists"][1], "Watchlist");
    assert!(variables.get("manga").is_none());
}

#[tokio::test]
async fn a_failed_mutation_is_not_retried() {
    let mock = Mock::serve_with_status(StatusCode::INTERNAL_SERVER_ERROR).await;
    let result = mock.client.delete_list_entry("test-token", 9001).await;
    assert!(result.is_err(), "the failure reaches the caller");
    assert_eq!(
        mock.count(),
        1,
        "no automatic retry, despite max_retries(3)"
    );
}

#[tokio::test]
async fn custom_list_names_come_from_list_options_including_empty_lists() {
    let mock = Mock::serve().await;
    let names = mock.client.custom_list_names("test-token").await.unwrap();

    assert_eq!(names.anime, vec!["Favourites", "Empty Shelf"]);
    assert_eq!(names.manga, vec!["Read Later"]);
    assert_eq!(names.for_kind(MediaKind::Anime), names.anime.as_slice());
    assert_eq!(mock.count(), 1);
    let query = mock.request(0)["query"].as_str().unwrap().to_owned();
    assert!(query.contains("mediaListOptions"));
    assert!(
        !query.contains("MediaListCollection"),
        "names are read without the collection"
    );
    assert_eq!(mock.auth(), vec!["Bearer test-token"]);
}
