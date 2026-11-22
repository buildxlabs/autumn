//! The catalog queries against recorded responses served from a loopback
//! mock. No test reaches the live AniList API.
//!
//! Fixtures in `tests/fixtures/` are real AniList responses recorded on
//! 2026-11-22, except `catalog_missed_sequels_collection*.json`, whose
//! list entries are assembled from those recordings to exercise the
//! missed-sequels filter.

#![expect(
    clippy::indexing_slicing,
    reason = "tests index known fixture data; a panic is the failed assertion"
)]

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use metadata::{AniList, MediaFormat, MediaSeason};
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// A mock AniList that answers each POST with the fixture for its query.
struct MockCatalog {
    client: AniList,
    requests: Arc<Mutex<Vec<Value>>>,
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture or failed bind cannot be recovered"
)]
impl MockCatalog {
    /// Serves the ordinary collection fixture.
    async fn serve() -> Self {
        Self::serve_collection("catalog_missed_sequels_collection.json").await
    }

    async fn serve_collection(collection: &'static str) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(seen): State<Arc<Mutex<Vec<Value>>>>,
                          Json(request): Json<Value>| async move {
                        seen.lock().unwrap().push(request.clone());
                        let query = request["query"].as_str().unwrap_or_default();
                        (StatusCode::OK, Json(load(query, collection)))
                    },
                ),
            )
            .with_state(Arc::clone(&requests));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // The loopback host gets AniList's error handling, without the real
        // rate limit or retries.
        let http = net::Client::builder()
            .host("127.0.0.1", 6000, 100, net::HostKind::AniList)
            .max_retries(0)
            .build()
            .unwrap();
        Self {
            client: AniList::with_endpoint(http, format!("http://{address}/")),
            requests,
        }
    }

    fn requests(&self) -> Vec<Value> {
        self.requests.lock().unwrap().clone()
    }

    fn request_count(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

/// The recorded response for a query, dispatched on the query document.
#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture cannot be recovered"
)]
fn load(query: &str, collection: &str) -> Value {
    let path = format!(
        "{}/tests/fixtures/{}",
        env!("CARGO_MANIFEST_DIR"),
        fixture_for(query, collection)
    );
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn fixture_for<'a>(query: &str, collection: &'a str) -> &'a str {
    if query.contains("MediaListCollection(") {
        collection
    } else if query.contains("id_in: $ids") {
        "catalog_missed_sequels_relations.json"
    } else if query.contains("airingSchedules(notYetAired") {
        "catalog_airing_schedule.json"
    } else if query.contains("airingSchedules(airingAt_lesser") {
        "catalog_aired_recently.json"
    } else if query.contains("seasonYear: $seasonYear") {
        "catalog_upcoming_season.json"
    } else if query.contains("countryOfOrigin: $country") {
        "catalog_trending_by_country.json"
    } else if query.contains("format: MOVIE") {
        "catalog_trending_movies.json"
    } else if query.contains("type: MANGA") {
        "catalog_trending_manga.json"
    } else if query.contains("sort: [POPULARITY_DESC]") {
        "catalog_popular.json"
    } else {
        "catalog_trending.json"
    }
}

/// The query document of one request, whitespace collapsed.
fn query_at(requests: &[Value], index: usize) -> String {
    requests[index]["query"]
        .as_str()
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

#[tokio::test]
async fn trending_decodes_the_anime_list() {
    let mock = MockCatalog::serve().await;

    let trending = mock.client.trending().await.unwrap();

    assert_eq!(trending.len(), 50);
    assert_eq!(trending[0].anilist_id, 195_604);
    assert_eq!(
        trending[0].title.romaji.as_deref(),
        Some("Black Clover 2nd Season")
    );
    assert_eq!(trending[1].anilist_id, 189_123);
}

#[tokio::test]
async fn popular_decodes_the_anime_list() {
    let mock = MockCatalog::serve().await;

    let popular = mock.client.popular().await.unwrap();

    assert_eq!(popular.len(), 50);
    assert_eq!(popular[0].anilist_id, 16_498);
    assert_eq!(
        popular[0].title.romaji.as_deref(),
        Some("Shingeki no Kyojin")
    );
    assert_eq!(popular[1].anilist_id, 101_922);
}

#[tokio::test]
async fn upcoming_season_decodes_the_anime_list() {
    let mock = MockCatalog::serve().await;

    let upcoming = mock.client.upcoming_season().await.unwrap();

    assert_eq!(upcoming.len(), 50);
    assert_eq!(upcoming[0].anilist_id, 177_704);
    assert_eq!(
        upcoming[0].title.romaji.as_deref(),
        Some("MASHLE: Sanma Taisou Shinkakusha Saishuu Shiken-hen")
    );
    assert_eq!(upcoming[0].season, Some(MediaSeason::Winter));

    let query = query_at(&mock.requests(), 0);
    assert!(
        query.contains("season: $season, seasonYear: $seasonYear"),
        "query: {query}"
    );
}

#[tokio::test]
async fn trending_movies_decodes_the_movies() {
    let mock = MockCatalog::serve().await;

    let movies = mock.client.trending_movies().await.unwrap();

    assert_eq!(movies.len(), 50);
    assert_eq!(movies[0].anilist_id, 171_627);
    assert_eq!(
        movies[0].title.romaji.as_deref(),
        Some("Chainsaw Man: Reze-hen")
    );
    assert_eq!(movies[0].format, Some(MediaFormat::Movie));
}

#[tokio::test]
async fn trending_manga_decodes_the_manga() {
    let mock = MockCatalog::serve().await;

    let manga = mock.client.trending_manga().await.unwrap();

    assert_eq!(manga.len(), 50);
    assert_eq!(manga[0].anilist_id, 211_510);
    assert_eq!(manga[0].format, Some(MediaFormat::Manga));
}

#[tokio::test]
async fn trending_by_country_decodes_the_japanese_trending() {
    let mock = MockCatalog::serve().await;

    let japan = mock.client.trending_by_country("JP").await.unwrap();

    assert_eq!(japan[0].anilist_id, 195_604);
    assert_eq!(mock.requests()[0]["variables"]["country"], "JP");
}

#[tokio::test]
async fn airing_schedule_decodes_the_upcoming_episodes() {
    let mock = MockCatalog::serve().await;

    let airing = mock.client.airing_schedule().await.unwrap();

    assert_eq!(airing.len(), 50);
    assert_eq!(airing[0].media.anilist_id, 218_217);
    assert_eq!(
        airing[0].media.title.romaji.as_deref(),
        Some("JK Dark Summoner to Deki......Dei Nai!?")
    );
    // Each entry keeps its own episode and air time: the fixture was
    // recorded with sort [TIME], so this is the next episode to air.
    assert_eq!(airing[0].episode, 8);
    assert_eq!(airing[0].airing_at, 1_795_370_400);
    assert_eq!(airing[0].time_until_airing, 4_240);
}

#[tokio::test]
async fn aired_recently_decodes_the_recent_episodes() {
    let mock = MockCatalog::serve().await;

    let recent = mock.client.aired_recently().await.unwrap();

    assert_eq!(recent.len(), 50);
    assert_eq!(
        recent[0].media.title.romaji.as_deref(),
        Some("Mahou no Shimai LuluttoLilly Part 2")
    );
    assert_eq!(recent[0].episode, 8);
    assert_eq!(recent[0].airing_at, 1_795_354_200);
    assert_eq!(recent[0].time_until_airing, -8_981);
}

#[tokio::test]
async fn missed_sequels_decodes_the_sequels_of_completed_entries() {
    let mock = MockCatalog::serve().await;

    let missed = mock.client.missed_sequels("kiyors").await.unwrap();

    assert_eq!(missed.len(), 1, "Shingeki no Kyojin Season 2 is the sequel");
    assert_eq!(missed[0].anilist_id, 20_958);
    assert_eq!(
        missed[0].title.romaji.as_deref(),
        Some("Shingeki no Kyojin Season 2")
    );
}

#[tokio::test]
async fn missed_sequels_excludes_media_already_on_the_list() {
    let mock = MockCatalog::serve_collection("catalog_missed_sequels_collection_listed.json").await;

    let missed = mock.client.missed_sequels("kiyors").await.unwrap();

    assert_eq!(missed.len(), 0, "the sequel is already on the collection");
}

#[tokio::test]
async fn a_home_load_uses_three_requests() {
    let mock = MockCatalog::serve().await;

    let home = mock.client.home_load().await.unwrap();

    assert_eq!(home.trending.len(), 50);
    assert_eq!(home.aired_recently.len(), 50);
    assert_eq!(home.anime_carousel.len(), 50);
    assert_eq!(
        mock.request_count(),
        3,
        "Home: Trending Anime, Aired Recently, anime carousel"
    );
}

#[tokio::test]
async fn a_discover_load_uses_eleven_requests() {
    let mock = MockCatalog::serve().await;

    let discover = mock.client.discover_load("kiyors").await.unwrap();

    assert_eq!(discover.trending.len(), 50);
    assert_eq!(discover.popular.len(), 50);
    assert_eq!(discover.upcoming_season.len(), 50);
    assert_eq!(discover.trending_movies.len(), 50);
    assert_eq!(discover.airing_schedule.len(), 50);
    assert_eq!(discover.missed_sequels.len(), 1);
    assert_eq!(discover.trending_manga.len(), 50);
    assert_eq!(discover.trending_by_country.japan.len(), 50);
    assert_eq!(discover.trending_by_country.korea.len(), 50);
    assert_eq!(discover.trending_by_country.china.len(), 50);
    assert_eq!(
        mock.request_count(),
        11,
        "Discover: Trending Anime, Popular, Upcoming Season, Trending Movies, \
         Airing Schedule, Missed Sequels (2), Trending Manga, \
         Trending by Country (JP, KR, CN)"
    );
}

#[tokio::test]
async fn catalog_queries_ask_for_the_first_page() {
    let mock = MockCatalog::serve().await;

    mock.client.trending().await.unwrap();

    assert_eq!(
        mock.requests()[0]["variables"],
        json!({"page": 1, "perPage": 50})
    );
}

#[tokio::test]
async fn airing_schedule_asks_for_not_yet_aired_episodes_in_airing_order() {
    let mock = MockCatalog::serve().await;

    mock.client.airing_schedule().await.unwrap();

    let query = query_at(&mock.requests(), 0);
    assert!(
        query.contains("airingSchedules(notYetAired: true, sort: [TIME])"),
        "query: {query}"
    );
}

#[tokio::test]
async fn aired_recently_asks_for_episodes_that_already_aired() {
    let mock = MockCatalog::serve().await;

    mock.client.aired_recently().await.unwrap();

    let query = query_at(&mock.requests(), 0);
    assert!(
        query.contains("airingSchedules(airingAt_lesser: $now, sort: [TIME_DESC])"),
        "query: {query}"
    );
    assert!(mock.requests()[0]["variables"]["now"].is_number());
}

#[tokio::test]
async fn missed_sequels_binds_the_user_name() {
    let mock = MockCatalog::serve().await;

    mock.client.missed_sequels("kiyors").await.unwrap();

    let requests = mock.requests();
    assert_eq!(requests[0]["variables"]["userName"], "kiyors");
    assert!(
        query_at(&requests, 0).contains("MediaListCollection("),
        "the collection is the first request"
    );
    assert!(query_at(&requests, 1).contains("id_in: $ids"));
}
