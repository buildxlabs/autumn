//! The AniList client against recorded responses served from a loopback mock.
//! No test reaches the live API.
//!
//! Fixtures in `tests/fixtures/` are real AniList responses recorded on
//! 2026-11-22 from the queries this crate builds, except
//! `search_poster_fallback.json`: two search items with `extraLarge` (and in
//! one, `large`) nulled to exercise the poster fallback.

#![expect(
    clippy::indexing_slicing,
    reason = "tests index known fixture data; a panic is the failed assertion"
)]

use std::sync::{Arc, Mutex};

use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
use domain::{ExternalId, FuzzyDate, MediaKind};
use metadata::{
    AniList, CharacterRole, Error, MediaFormat, MediaRelation, MediaSeason, MediaSort, MediaStatus,
    PageRequest, SearchFilters,
};
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// A mock AniList that answers every POST with one fixture and keeps each
/// request body.
struct Mock {
    client: AniList,
    requests: Arc<Mutex<Vec<Value>>>,
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture or failed bind cannot be recovered"
)]
impl Mock {
    async fn serve(status: StatusCode, fixture: &str) -> Self {
        let path = format!("{}/tests/fixtures/{fixture}", env!("CARGO_MANIFEST_DIR"));
        let body = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
        Self::serve_json(status, body).await
    }

    async fn serve_json(status: StatusCode, body: Value) -> Self {
        let requests = Arc::new(Mutex::new(Vec::new()));
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(seen): State<Arc<Mutex<Vec<Value>>>>,
                          Json(request): Json<Value>| async move {
                        seen.lock().unwrap().push(request);
                        (status, Json(body))
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
        let client = AniList::with_endpoint(http, format!("http://{address}/"));
        Self { client, requests }
    }

    fn variables(&self) -> Value {
        let requests = self.requests.lock().unwrap();
        assert_eq!(requests.len(), 1, "expected exactly one request");
        requests[0]["variables"].clone()
    }
}

#[tokio::test]
async fn media_detail_decodes_the_header_fields() {
    let mock = Mock::serve(StatusCode::OK, "media_detail_154587.json").await;

    let detail = mock.client.media(154_587).await.unwrap();

    let summary = &detail.summary;
    assert_eq!(summary.anilist_id, 154_587);
    assert_eq!(summary.mal_id, Some(52_991));
    assert_eq!(summary.kind, Some(MediaKind::Anime));
    assert_eq!(summary.title.romaji.as_deref(), Some("Sousou no Frieren"));
    assert_eq!(summary.title.native.as_deref(), Some("葬送のフリーレン"));
    assert_eq!(summary.format, Some(MediaFormat::Tv));
    assert_eq!(summary.status, Some(MediaStatus::Finished));
    assert_eq!(summary.season, Some(MediaSeason::Fall));
    assert_eq!(summary.season_year, Some(2023));
    assert_eq!(summary.episodes, Some(28));
    assert_eq!(summary.mean_score, Some(90));
    assert_eq!(&summary.genres[..3], ["Adventure", "Drama", "Fantasy"]);
    assert!(!summary.is_adult, "Frieren is not adult");
    assert_eq!(
        summary.poster.as_deref(),
        Some(
            "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx154587-qQTzQnEJJ3oB.jpg"
        ),
        "poster is coverImage.extraLarge"
    );
    assert!(summary.banner.is_some(), "Frieren has a banner");
    assert_eq!(
        summary.external_ids(),
        [ExternalId::AniList(154_587), ExternalId::Mal(52_991)]
    );

    assert!(
        detail
            .description
            .as_deref()
            .unwrap()
            .starts_with("The adventure is over but life goes on")
    );
    assert_eq!(
        detail.start_date,
        FuzzyDate::new(Some(2023), Some(9), Some(29)).unwrap()
    );
    assert_eq!(
        detail.end_date,
        FuzzyDate::new(Some(2024), Some(3), Some(22)).unwrap()
    );
    assert_eq!(detail.duration, Some(24));
    assert_eq!(detail.popularity, Some(501_354));
    assert_eq!(detail.studios, ["MADHOUSE"]);
}

#[tokio::test]
async fn media_detail_decodes_characters_relations_and_recommendations() {
    let mock = Mock::serve(StatusCode::OK, "media_detail_154587.json").await;

    let detail = mock.client.media(154_587).await.unwrap();

    assert_eq!(detail.characters.len(), 25);
    let frieren = &detail.characters[0];
    assert_eq!(frieren.anilist_id, 176_754);
    assert_eq!(frieren.name.as_deref(), Some("Frieren"));
    assert_eq!(frieren.role, Some(CharacterRole::Main));
    assert_eq!(
        frieren.image.as_deref(),
        Some("https://s4.anilist.co/file/anilistcdn/character/large/b176754-PCnpqIOkjhFk.png")
    );
    let actor = &frieren.voice_actors[0];
    assert_eq!(actor.anilist_id, 112_215);
    assert_eq!(actor.name.as_deref(), Some("Atsumi Tanezaki"));
    assert_eq!(detail.characters[3].role, Some(CharacterRole::Supporting));

    let relations: Vec<_> = detail
        .relations
        .iter()
        .map(|relation| (relation.relation.clone(), relation.media.anilist_id))
        .collect();
    assert_eq!(
        relations,
        [
            (Some(MediaRelation::Source), 118_586),
            (Some(MediaRelation::Character), 169_811),
            (Some(MediaRelation::SideStory), 170_068),
            (Some(MediaRelation::Other), 175_691),
            (Some(MediaRelation::Sequel), 182_255),
            (Some(MediaRelation::SideStory), 189_513),
        ]
    );
    assert_eq!(detail.relations[0].media.kind, Some(MediaKind::Manga));

    // 25 were fetched; AniList sent one with a null media, which is dropped.
    assert_eq!(detail.recommendations.len(), 24);
    assert_eq!(detail.recommendations[0].anilist_id, 21_827);
}

#[tokio::test]
async fn media_detail_asks_for_the_requested_id() {
    let mock = Mock::serve(StatusCode::OK, "media_detail_154587.json").await;

    mock.client.media(154_587).await.unwrap();

    assert_eq!(mock.variables()["id"], 154_587);
}

#[tokio::test]
async fn a_missing_media_is_not_found() {
    // AniList answers a missing id with HTTP 404 and a GraphQL error.
    let mock = Mock::serve(StatusCode::NOT_FOUND, "media_not_found.json").await;

    let error = mock.client.media(999_999_999).await.unwrap_err();

    assert!(matches!(error, Error::NotFound), "got {error:?}");
}

#[tokio::test]
async fn a_graphql_error_on_http_200_is_an_api_error() {
    // AniList's documented shape for a query that needs a token.
    let body = json!({"errors": [{"message": "Unauthorized.", "status": 401}], "data": null});
    let mock = Mock::serve_json(StatusCode::OK, body).await;

    let error = mock.client.media(1).await.unwrap_err();

    assert!(
        matches!(error, Error::Api { status: Some(401), ref message } if message == "Unauthorized."),
        "got {error:?}"
    );
}

#[tokio::test]
async fn an_outage_is_anilist_unavailable() {
    let mock = Mock::serve(StatusCode::FORBIDDEN, "media_not_found.json").await;

    let error = mock.client.media(1).await.unwrap_err();

    assert!(
        matches!(error, Error::Net(net::Error::AniListUnavailable)),
        "got {error:?}"
    );
}

#[tokio::test]
async fn search_decodes_a_page_including_media_without_a_banner() {
    let mock = Mock::serve(StatusCode::OK, "search_frieren.json").await;

    let page = mock
        .client
        .search(&SearchFilters::default(), PageRequest::default())
        .await
        .unwrap();

    let ids: Vec<_> = page.media.iter().map(|media| media.anilist_id).collect();
    assert_eq!(ids, [154_587, 209_939, 182_255, 170_068, 189_513, 206_425]);
    assert!(!page.has_next_page, "one page of results");
    let third_season = &page.media[1];
    assert_eq!(third_season.banner, None, "3rd Season has no banner");
    assert_eq!(page.media[3].format, Some(MediaFormat::Ona));
}

#[tokio::test]
async fn search_falls_back_to_large_then_medium_posters() {
    let mock = Mock::serve(StatusCode::OK, "search_poster_fallback.json").await;

    let page = mock
        .client
        .search(&SearchFilters::default(), PageRequest::default())
        .await
        .unwrap();

    assert!(page.has_next_page, "fixture says another page follows");
    assert_eq!(
        page.media[0].poster.as_deref(),
        Some(
            "https://s4.anilist.co/file/anilistcdn/media/anime/cover/medium/bx170068-ijY3tCP8KoWP.jpg"
        ),
        "no extraLarge: use large"
    );
    assert_eq!(
        page.media[1].poster.as_deref(),
        Some(
            "https://s4.anilist.co/file/anilistcdn/media/anime/cover/small/bx189513-1R9Sryve0K53.png"
        ),
        "no extraLarge or large: use medium"
    );
}

#[tokio::test]
async fn search_caps_the_page_size_at_50() {
    let mock = Mock::serve(StatusCode::OK, "search_frieren.json").await;

    mock.client
        .search(&SearchFilters::default(), PageRequest::new(3, 100))
        .await
        .unwrap();

    let variables = mock.variables();
    assert_eq!(variables["page"], 3);
    assert_eq!(variables["perPage"], 50);
}

#[test]
fn page_requests_are_clamped_to_anilists_limits() {
    let page = PageRequest::new(0, 0);
    assert_eq!((page.page(), page.per_page()), (1, 1));
    let page = PageRequest::new(2, 500);
    assert_eq!((page.page(), page.per_page()), (2, 50));
    let page = PageRequest::default();
    assert_eq!((page.page(), page.per_page()), (1, 50));
}

#[tokio::test]
async fn unset_filters_are_left_out_and_adult_media_is_hidden() {
    let mock = Mock::serve(StatusCode::OK, "search_frieren.json").await;

    mock.client
        .search(&SearchFilters::default(), PageRequest::default())
        .await
        .unwrap();

    assert_eq!(
        mock.variables(),
        json!({"page": 1, "perPage": 50, "isAdult": false})
    );
}

#[tokio::test]
async fn every_filter_serializes_as_anilist_expects() {
    let mock = Mock::serve(StatusCode::OK, "search_frieren.json").await;
    let filters = SearchFilters {
        search: Some("frieren".to_owned()),
        kind: Some(MediaKind::Anime),
        sort: Some(MediaSort::SearchMatch),
        format: Some(MediaFormat::TvShort),
        status: Some(MediaStatus::NotYetReleased),
        season: Some(MediaSeason::Fall),
        season_year: Some(2023),
        genres: vec!["Adventure".to_owned(), "Slice of Life".to_owned()],
        country_of_origin: Some("JP".to_owned()),
        is_adult: true,
    };

    mock.client
        .search(&filters, PageRequest::new(2, 20))
        .await
        .unwrap();

    assert_eq!(
        mock.variables(),
        json!({
            "page": 2,
            "perPage": 20,
            "search": "frieren",
            "type": "ANIME",
            "sort": ["SEARCH_MATCH"],
            "format": "TV_SHORT",
            "status": "NOT_YET_RELEASED",
            "season": "FALL",
            "seasonYear": 2023,
            "genreIn": ["Adventure", "Slice of Life"],
            "countryOfOrigin": "JP",
            "isAdult": true,
        })
    );
}

#[tokio::test]
async fn an_unlisted_sort_is_sent_as_spelled() {
    let mock = Mock::serve(StatusCode::OK, "search_frieren.json").await;
    let filters = SearchFilters {
        sort: Some(MediaSort::Other("TITLE_ROMAJI".to_owned())),
        ..SearchFilters::default()
    };

    mock.client
        .search(&filters, PageRequest::default())
        .await
        .unwrap();

    assert_eq!(mock.variables()["sort"], json!(["TITLE_ROMAJI"]));
}
