//! The AniList catalog behind Advanced Search (ticket #59).
//!
//! The session in `ui-state` owns the request-counting logic and knows no
//! AniList types; this adapter maps the UI filter set onto the GraphQL
//! client's filters and its results back into plain view models.

use metadata::{
    AniList, MediaFormat, MediaSeason, MediaSort, MediaStatus, MediaSummary, PageRequest,
};
use ui_state::{
    Catalog, CatalogError, MediaPreview, SearchFilters, SearchFormat, SearchPage, SearchSeason,
    SearchSort, SearchStatus,
};

use crate::palette_search::display_label;

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
fn sort(filter: SearchSort) -> MediaSort {
    match filter {
        SearchSort::Match => MediaSort::SearchMatch,
        SearchSort::Trending => MediaSort::TrendingDesc,
        SearchSort::Popularity => MediaSort::PopularityDesc,
        SearchSort::Score => MediaSort::ScoreDesc,
        SearchSort::Favourites => MediaSort::FavouritesDesc,
        SearchSort::StartDate => MediaSort::StartDateDesc,
        SearchSort::UpdatedAt => MediaSort::UpdatedAtDesc,
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
fn format(filter: SearchFormat) -> MediaFormat {
    match filter {
        SearchFormat::Tv => MediaFormat::Tv,
        SearchFormat::TvShort => MediaFormat::TvShort,
        SearchFormat::Movie => MediaFormat::Movie,
        SearchFormat::Special => MediaFormat::Special,
        SearchFormat::Ova => MediaFormat::Ova,
        SearchFormat::Ona => MediaFormat::Ona,
        SearchFormat::Music => MediaFormat::Music,
        SearchFormat::Manga => MediaFormat::Manga,
        SearchFormat::Novel => MediaFormat::Novel,
        SearchFormat::OneShot => MediaFormat::OneShot,
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
fn status(filter: SearchStatus) -> MediaStatus {
    match filter {
        SearchStatus::Releasing => MediaStatus::Releasing,
        SearchStatus::Finished => MediaStatus::Finished,
        SearchStatus::NotYetReleased => MediaStatus::NotYetReleased,
        SearchStatus::Cancelled => MediaStatus::Cancelled,
        SearchStatus::Hiatus => MediaStatus::Hiatus,
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
fn season(filter: SearchSeason) -> MediaSeason {
    match filter {
        SearchSeason::Winter => MediaSeason::Winter,
        SearchSeason::Spring => MediaSeason::Spring,
        SearchSeason::Summer => MediaSeason::Summer,
        SearchSeason::Fall => MediaSeason::Fall,
    }
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
fn preview(media: &MediaSummary) -> MediaPreview {
    let format = match &media.format {
        Some(MediaFormat::Tv) => Some(SearchFormat::Tv),
        Some(MediaFormat::TvShort) => Some(SearchFormat::TvShort),
        Some(MediaFormat::Movie) => Some(SearchFormat::Movie),
        Some(MediaFormat::Special) => Some(SearchFormat::Special),
        Some(MediaFormat::Ova) => Some(SearchFormat::Ova),
        Some(MediaFormat::Ona) => Some(SearchFormat::Ona),
        Some(MediaFormat::Music) => Some(SearchFormat::Music),
        Some(MediaFormat::Manga) => Some(SearchFormat::Manga),
        Some(MediaFormat::Novel) => Some(SearchFormat::Novel),
        Some(MediaFormat::OneShot) => Some(SearchFormat::OneShot),
        _ => None,
    };
    MediaPreview {
        anilist_id: media.anilist_id,
        title: display_label(media),
        poster: media.poster.clone(),
        format,
        score: media.mean_score,
    }
}

/// Searches AniList through the stale-first client's underlying catalog.
#[derive(Debug, Clone)]
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
pub(crate) struct AniListCatalog {
    anilist: AniList,
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "wired into the Search screen when the poster grid lands (#55)"
    )
)]
impl AniListCatalog {
    /// Uses `anilist` for every page request.
    pub(crate) fn new(anilist: AniList) -> Self {
        Self { anilist }
    }
}

impl Catalog for AniListCatalog {
    async fn search(&self, filters: &SearchFilters, page: u32) -> Result<SearchPage, CatalogError> {
        let request = metadata::SearchFilters {
            search: filters.query.clone(),
            kind: filters.kind,
            sort: filters.sort.map(sort),
            format: filters.format.map(format),
            status: filters.status.map(status),
            season: filters.season.map(season),
            season_year: filters.year,
            genres: filters.genres.clone(),
            country_of_origin: filters.country.clone(),
            is_adult: filters.adult,
        };
        let page = self
            .anilist
            .search(&request, PageRequest::new(page, PageRequest::MAX_PER_PAGE))
            .await
            .map_err(|error| CatalogError::new(error.to_string()))?;
        Ok(SearchPage {
            results: page.media.iter().map(preview).collect(),
            has_next_page: page.has_next_page,
        })
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "test assertions index the captured JSON variables by key"
    )]

    use std::sync::{Arc, Mutex};

    use axum::{Json, Router, extract::State, routing::post};
    use domain::MediaKind;
    use serde_json::Value;
    use tokio::net::TcpListener;

    use super::AniListCatalog;
    use ui_state::{Catalog, SearchFilters, SearchFormat, SearchSeason, SearchSort, SearchStatus};

    #[tokio::test]
    async fn every_decision_23_filter_reaches_anilist() {
        let body: Value = serde_json::from_str(
            &std::fs::read_to_string(format!(
                "{}/../metadata/tests/fixtures/search_frieren.json",
                env!("CARGO_MANIFEST_DIR")
            ))
            .expect("fixture"),
        )
        .expect("valid JSON");
        let response = body.clone();
        let seen = Arc::new(Mutex::new(Value::Null));
        let recorder = Arc::clone(&seen);
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(recorder): State<Arc<Mutex<Value>>>, Json(request): Json<Value>| {
                        let response = response.clone();
                        async move {
                            *recorder.lock().expect("unpoisoned") = request;
                            Json(response)
                        }
                    },
                ),
            )
            .with_state(recorder);
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move {
            axum::serve(listener, app).await.expect("serve");
        });

        let http = net::Client::builder()
            .max_retries(0)
            .build()
            .expect("client");
        let anilist = metadata::AniList::with_endpoint(http, format!("http://{address}/"));
        let catalog = AniListCatalog::new(anilist);
        let filters = SearchFilters {
            query: Some("frieren".to_owned()),
            kind: Some(MediaKind::Anime),
            sort: Some(SearchSort::Score),
            format: Some(SearchFormat::Tv),
            status: Some(SearchStatus::Releasing),
            season: Some(SearchSeason::Fall),
            year: Some(2023),
            genres: vec!["Fantasy".to_owned(), "Drama".to_owned()],
            country: Some("JP".to_owned()),
            adult: true,
        };

        // The response is irrelevant to this test; every filter must be in
        // the variables AniList receives.
        let _ = catalog.search(&filters, 1).await;
        let variables = seen.lock().expect("unpoisoned")["variables"].clone();
        for key in [
            "search",
            "type",
            "sort",
            "format",
            "status",
            "season",
            "seasonYear",
            "genreIn",
            "countryOfOrigin",
            "isAdult",
        ] {
            assert!(!variables[key].is_null(), "missing filter {key}");
        }
        assert_eq!(variables["search"], "frieren");
        assert_eq!(variables["type"], "ANIME");
        assert_eq!(variables["sort"][0], "SCORE_DESC");
        assert_eq!(variables["format"], "TV");
        assert_eq!(variables["status"], "RELEASING");
        assert_eq!(variables["season"], "FALL");
        assert_eq!(variables["seasonYear"], 2023);
        assert_eq!(variables["genreIn"][0], "Fantasy");
        assert_eq!(variables["countryOfOrigin"], "JP");
        assert_eq!(variables["isAdult"], true);
    }
}
