//! The paged Advanced Search query.

use domain::MediaKind;

use crate::{
    AniList, Error, MediaFormat, MediaSeason, MediaStatus, MediaSummary,
    fragments::{CountryCode, MediaCard, MediaType},
    schema,
};

/// A page of results to fetch. AniList returns at most 50 items per page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PageRequest {
    page: u32,
    per_page: u32,
}

impl PageRequest {
    /// The most items AniList returns in one page.
    pub const MAX_PER_PAGE: u32 = 50;

    /// Page `page` (counting from 1) of `per_page` items. `per_page` is
    /// capped at [`Self::MAX_PER_PAGE`]; zeros become 1.
    #[must_use]
    pub fn new(page: u32, per_page: u32) -> Self {
        Self {
            page: page.max(1),
            per_page: per_page.clamp(1, Self::MAX_PER_PAGE),
        }
    }

    /// The page number, from 1.
    #[must_use]
    pub fn page(self) -> u32 {
        self.page
    }

    /// Items per page, 1–50.
    #[must_use]
    pub fn per_page(self) -> u32 {
        self.per_page
    }
}

impl Default for PageRequest {
    /// The first page of 50.
    fn default() -> Self {
        Self::new(1, Self::MAX_PER_PAGE)
    }
}

/// The Advanced Search filters. An unset filter matches everything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct SearchFilters {
    /// Free-text search over titles.
    pub search: Option<String>,
    /// Anime or manga.
    pub kind: Option<MediaKind>,
    /// Result order. AniList orders by id when unset.
    pub sort: Option<MediaSort>,
    /// Release format.
    pub format: Option<MediaFormat>,
    /// Releasing status.
    pub status: Option<MediaStatus>,
    /// The season first aired in.
    pub season: Option<MediaSeason>,
    /// The year of that season.
    pub season_year: Option<u16>,
    /// Every one of these genres must match.
    pub genres: Vec<String>,
    /// ISO 3166-1 alpha-2 country of origin, such as `JP`.
    pub country_of_origin: Option<String>,
    /// `false` (the default) hides adult media; `true` shows only adult media.
    pub is_adult: bool,
}

/// Search result order (AniList's `MediaSort`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum MediaSort {
    /// Closest title match first; use with a search text.
    SearchMatch,
    /// Most active in the past hour first.
    TrendingDesc,
    /// On the most lists first.
    PopularityDesc,
    /// Highest scored first.
    ScoreDesc,
    /// Most favourited first.
    FavouritesDesc,
    /// Newest first.
    StartDateDesc,
    /// Most recently edited on AniList first.
    UpdatedAtDesc,
    /// Any other AniList sort, as AniList spells it (`TITLE_ROMAJI`).
    #[cynic(fallback)]
    Other(String),
}

/// One page of search results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    /// The matching media on this page.
    pub media: Vec<MediaSummary>,
    /// Whether another page follows. AniList's totals are unreliable, so
    /// this is the only paging signal.
    pub has_next_page: bool,
}

impl AniList {
    /// Search the catalog with the Advanced Search filters.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails (including AniList's rate limit
    /// and outages), [`Error::Api`] for a GraphQL error, or [`Error::Decode`]
    /// for an unexpected response.
    pub async fn search(
        &self,
        filters: &SearchFilters,
        page: PageRequest,
    ) -> Result<SearchPage, Error> {
        use cynic::QueryBuilder;

        let operation = SearchQuery::build(SearchVariables::new(filters, page));
        let page = self.query(operation).await?.page.unwrap_or_default();
        Ok(SearchPage {
            media: page
                .media
                .into_iter()
                .flatten()
                .flatten()
                .filter_map(MediaSummary::from_card)
                .collect(),
            has_next_page: page
                .page_info
                .and_then(|info| info.has_next_page)
                .unwrap_or_default(),
        })
    }
}

#[derive(cynic::QueryVariables, Debug)]
struct SearchVariables {
    page: i32,
    per_page: i32,
    #[cynic(skip_serializing_if = "Option::is_none")]
    search: Option<String>,
    #[cynic(skip_serializing_if = "Option::is_none", rename = "type")]
    kind: Option<MediaType>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    sort: Option<Vec<MediaSort>>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    format: Option<MediaFormat>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    status: Option<MediaStatus>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    season: Option<MediaSeason>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    season_year: Option<i32>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    genre_in: Option<Vec<String>>,
    #[cynic(skip_serializing_if = "Option::is_none")]
    country_of_origin: Option<CountryCode>,
    is_adult: bool,
}

impl SearchVariables {
    fn new(filters: &SearchFilters, page: PageRequest) -> Self {
        let int = |value: u32| i32::try_from(value).unwrap_or(i32::MAX);
        Self {
            page: int(page.page()),
            per_page: int(page.per_page()),
            search: filters.search.clone(),
            kind: filters.kind.and_then(MediaType::from_kind),
            sort: filters.sort.clone().map(|sort| vec![sort]),
            format: filters.format.clone(),
            status: filters.status.clone(),
            season: filters.season.clone(),
            season_year: filters.season_year.map(i32::from),
            genre_in: (!filters.genres.is_empty()).then(|| filters.genres.clone()),
            country_of_origin: filters.country_of_origin.clone().map(CountryCode),
            is_adult: filters.is_adult,
        }
    }
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "SearchVariables")]
struct SearchQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<Page>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(variables = "SearchVariables")]
struct Page {
    page_info: Option<PageInfo>,
    #[arguments(
        search: $search,
        type: $kind,
        sort: $sort,
        format: $format,
        status: $status,
        season: $season,
        seasonYear: $season_year,
        genre_in: $genre_in,
        countryOfOrigin: $country_of_origin,
        isAdult: $is_adult
    )]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryFragment, Debug)]
struct PageInfo {
    has_next_page: Option<bool>,
}
