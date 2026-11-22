//! The catalog queries behind Home and Discover (#23).

use std::collections::HashSet;
use std::time::{SystemTime, UNIX_EPOCH};

use cynic::QueryBuilder;

use crate::{
    AniList, Error, MediaFormat, MediaRelation, MediaSeason, MediaStatus, MediaSummary,
    PageRequest,
    fragments::{CountryCode, CoverImage, MediaCard, MediaType, Title, count},
    schema,
};

/// One episode in an airing schedule: the show and the episode's air time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AiringEpisode {
    /// The show the episode belongs to.
    pub media: MediaSummary,
    /// The episode number of the release.
    pub episode: u32,
    /// When the episode will air or did air, as Unix seconds.
    pub airing_at: i32,
    /// Seconds until the episode starts; negative once it has aired.
    pub time_until_airing: i32,
}

impl AiringEpisode {
    /// The episode of a schedule entry: its own number and air time.
    fn from_entry(node: ScheduleEntry) -> Option<Self> {
        Some(Self {
            media: MediaSummary::from_airing(node.media?)?,
            episode: count(node.episode)?,
            airing_at: node.airing_at,
            time_until_airing: node.time_until_airing,
        })
    }
}

/// What the Home screen loads: Trending Anime, Aired Recently and the
/// anime carousel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HomeLoad {
    /// The Trending Anime hero (Home's `discover-header` widget).
    pub trending: Vec<MediaSummary>,
    /// Global carousel of anime episodes that aired recently
    /// (Home's `aired-recently` widget).
    pub aired_recently: Vec<AiringEpisode>,
    /// The anime carousel (Home's `anime-carousel` widget): Popular Anime.
    pub anime_carousel: Vec<MediaSummary>,
}

/// What the Discover screen loads: its eight carousels (#23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoverLoad {
    /// Trending Anime (the hero carousel).
    pub trending: Vec<MediaSummary>,
    /// Popular Anime.
    pub popular: Vec<MediaSummary>,
    /// Upcoming Season.
    pub upcoming_season: Vec<MediaSummary>,
    /// Trending Movies.
    pub trending_movies: Vec<MediaSummary>,
    /// Airing Schedule: upcoming episode releases.
    pub airing_schedule: Vec<AiringEpisode>,
    /// Missed Sequels: sequels of the user's completed anime entries that
    /// aren't on their collection yet.
    pub missed_sequels: Vec<MediaSummary>,
    /// Trending Manga (All).
    pub trending_manga: Vec<MediaSummary>,
    /// Trending by Country: the Japan, South Korea and China carousels.
    pub trending_by_country: CountryTrends,
}

/// Trending Anime by country of origin: Discover's "Trending by Country"
/// carousels, for JP, KR and CN (#23).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CountryTrends {
    /// The Japan (`JP`) carousel.
    pub japan: Vec<MediaSummary>,
    /// The South Korea (`KR`) carousel.
    pub korea: Vec<MediaSummary>,
    /// The China (`CN`) carousel.
    pub china: Vec<MediaSummary>,
}

impl AniList {
    /// Trending Anime: the catalog's most active anime.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn trending(&self) -> Result<Vec<MediaSummary>, Error> {
        let query = self.query(TrendingQuery::build(first_page())).await?;
        Ok(cards(query.page.and_then(|page| page.media)))
    }

    /// Popular Anime: the anime with the most list entries.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn popular(&self) -> Result<Vec<MediaSummary>, Error> {
        let query = self.query(PopularQuery::build(first_page())).await?;
        Ok(cards(query.page.and_then(|page| page.media)))
    }

    /// Upcoming Season: the anime of the next season, popular first.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn upcoming_season(&self) -> Result<Vec<MediaSummary>, Error> {
        let query = self
            .query(UpcomingSeasonQuery::build(UpcomingSeasonVariables::new()))
            .await?;
        Ok(cards(query.page.and_then(|page| page.media)))
    }

    /// Trending Movies.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn trending_movies(&self) -> Result<Vec<MediaSummary>, Error> {
        let query = self.query(TrendingMoviesQuery::build(first_page())).await?;
        Ok(cards(query.page.and_then(|page| page.media)))
    }

    /// Trending Manga.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn trending_manga(&self) -> Result<Vec<MediaSummary>, Error> {
        let query = self.query(TrendingMangaQuery::build(first_page())).await?;
        Ok(cards(query.page.and_then(|page| page.media)))
    }

    /// Trending Anime in one country of origin (`JP`, `KR` or `CN`).
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn trending_by_country(&self, country: &str) -> Result<Vec<MediaSummary>, Error> {
        let query = self
            .query(TrendingByCountryQuery::build(
                TrendingByCountryVariables::new(country),
            ))
            .await?;
        Ok(cards(query.page.and_then(|page| page.media)))
    }

    /// Airing Schedule: upcoming episode releases.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn airing_schedule(&self) -> Result<Vec<AiringEpisode>, Error> {
        let query = self.query(AiringScheduleQuery::build(first_page())).await?;
        Ok(query
            .page
            .and_then(|page| page.airing_schedules)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(AiringEpisode::from_entry)
            .collect())
    }

    /// Aired Recently: global carousel of anime episodes that aired recently.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when the request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn aired_recently(&self) -> Result<Vec<AiringEpisode>, Error> {
        let query = self
            .query(AiredRecentlyQuery::build(AiredRecentlyVariables::new()))
            .await?;
        Ok(query
            .page
            .and_then(|page| page.airing_schedules)
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(AiringEpisode::from_entry)
            .collect())
    }

    /// Missed Sequels: the sequels of the user's completed anime entries
    /// that aren't on their collection yet.
    ///
    /// `user_name` is the user's AniList username; their collection is the
    /// user data this reads. Reads the collection, then one page of media
    /// relations: two requests.
    ///
    /// # Errors
    ///
    /// [`Error::Net`] when a request fails, [`Error::Api`] for a GraphQL
    /// error, or [`Error::Decode`] for an unexpected response.
    pub async fn missed_sequels(&self, user_name: &str) -> Result<Vec<MediaSummary>, Error> {
        let collection = self
            .query(CollectionQuery::build(CollectionVariables::new(user_name)))
            .await?;

        let mut listed = HashSet::new();
        let mut completed = Vec::new();
        for group in collection
            .media_list_collection
            .into_iter()
            .filter_map(|collection| collection.lists)
            .flatten()
            .flatten()
        {
            for entry in group.entries.into_iter().flatten().flatten() {
                if let Some(id) = count(entry.media_id) {
                    listed.insert(id);
                }
                if entry.status == Some(MediaListStatus::Completed) {
                    completed.push(entry.media_id);
                }
            }
        }

        let relations = self
            .query(RelationsQuery::build(RelationsVariables::new(completed)))
            .await?;

        let mut missed: Vec<MediaSummary> = Vec::new();
        let mut seen = HashSet::new();
        for media in relations
            .page
            .and_then(|page| page.media)
            .into_iter()
            .flatten()
            .flatten()
        {
            for edge in media
                .relations
                .into_iter()
                .filter_map(|connection| connection.edges)
                .flatten()
                .flatten()
            {
                if edge.relation_type != Some(MediaRelation::Sequel) {
                    continue;
                }
                if let Some(summary) = edge.node.and_then(MediaSummary::from_card) {
                    let id = summary.anilist_id;
                    if !listed.contains(&id) && seen.insert(id) {
                        missed.push(summary);
                    }
                }
            }
        }
        Ok(missed)
    }

    /// What the Home screen loads: Trending Anime, Aired Recently and the
    /// anime carousel (#23).
    ///
    /// # Errors
    ///
    /// Any of the three requests failing; see the individual queries.
    pub async fn home_load(&self) -> Result<HomeLoad, Error> {
        Ok(HomeLoad {
            trending: self.trending().await?,
            aired_recently: self.aired_recently().await?,
            anime_carousel: self.popular().await?,
        })
    }

    /// What the Discover screen loads: its eight carousels (#23): 11
    /// requests — Trending Anime, Popular, Upcoming Season, Trending
    /// Movies, Airing Schedule, Missed Sequels (2), Trending Manga and
    /// Trending by Country (JP, KR, CN).
    ///
    /// `missed_sequels_user` is the AniList username whose collection feeds
    /// the Missed Sequels carousel.
    ///
    /// # Errors
    ///
    /// Any of the requests failing; see the individual queries.
    pub async fn discover_load(&self, missed_sequels_user: &str) -> Result<DiscoverLoad, Error> {
        Ok(DiscoverLoad {
            trending: self.trending().await?,
            popular: self.popular().await?,
            upcoming_season: self.upcoming_season().await?,
            trending_movies: self.trending_movies().await?,
            airing_schedule: self.airing_schedule().await?,
            missed_sequels: self.missed_sequels(missed_sequels_user).await?,
            trending_manga: self.trending_manga().await?,
            trending_by_country: CountryTrends {
                japan: self.trending_by_country("JP").await?,
                korea: self.trending_by_country("KR").await?,
                china: self.trending_by_country("CN").await?,
            },
        })
    }
}

/// The media of a `Page` media query.
fn cards(media: Option<Vec<Option<MediaCard>>>) -> Vec<MediaSummary> {
    media
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(MediaSummary::from_card)
        .collect()
}

/// The first page of the catalog queries: page 1, 50 per page.
fn first_page() -> PageVariables {
    let first = PageRequest::default();
    PageVariables {
        page: i32::try_from(first.page()).unwrap_or(1),
        per_page: i32::try_from(first.per_page()).unwrap_or(50),
    }
}

/// Now, as Unix seconds.
fn now() -> i32 {
    i32::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or_default(|elapsed| elapsed.as_secs()),
    )
    .unwrap_or(i32::MAX)
}

/// The upcoming season and its year from Unix-second `now`: the season
/// after the current one, where the WINTER season of year Y starts in
/// December.
fn upcoming_season(now: i32) -> (MediaSeason, i32) {
    let days = i64::from(now).div_euclid(SECONDS_PER_DAY);
    let (year, month, _day) = civil_from_days(days);
    match month {
        1..=2 => (MediaSeason::Spring, year),
        3..=5 => (MediaSeason::Summer, year),
        6..=8 => (MediaSeason::Fall, year),
        9..=11 => (MediaSeason::Winter, year + 1),
        // December already starts the Winter season, so next is Spring.
        _ => (MediaSeason::Spring, year + 1),
    }
}

/// Seconds in a day.
const SECONDS_PER_DAY: i64 = 86_400;
/// Days in a 400-year Gregorian cycle, from Howard Hinnant's
/// `civil_from_days` algorithm (public domain).
const DAYS_PER_ERA: i64 = 146_097;
/// Shifts the Unix epoch onto a multiple of that cycle.
const EPOCH_SHIFT: i64 = 719_468;

/// The (year, month, day) of a day count since the Unix epoch. The other
/// divisors are the leap-year and month-cycle lengths of Hinnant's
/// `civil_from_days` algorithm.
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + EPOCH_SHIFT;
    let era = z.div_euclid(DAYS_PER_ERA);
    let doe = z - era * DAYS_PER_ERA;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    (
        i32::try_from(year).unwrap_or(i32::MAX),
        u32::try_from(month).unwrap_or_default(),
        u32::try_from(day).unwrap_or_default(),
    )
}

/// AniList's `MediaListStatus`: a user's status for one entry.
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum MediaListStatus {
    /// Currently watching/reading (`CURRENT`).
    Current,
    /// Planning to watch/read (`PLANNING`).
    Planning,
    /// Finished watching/reading (`COMPLETED`).
    Completed,
    /// Stopped watching/reading before completing (`DROPPED`).
    Dropped,
    /// Paused (`PAUSED`).
    Paused,
    /// Re-watching/reading (`REPEATING`).
    Repeating,
    /// A status this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Other(String),
}

/// One episode in the Global airing schedule: the schedule entry plus its
/// show.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "AiringSchedule")]
struct ScheduleEntry {
    episode: i32,
    airing_at: i32,
    time_until_airing: i32,
    media: Option<AiringMedia>,
}

/// The show of a schedule entry. A separate fragment because `MediaCard`'s
/// conversion is infallible while an airing needs its show to be identifiable.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Media")]
struct AiringMedia {
    id: i32,
    id_mal: Option<i32>,
    #[cynic(rename = "type")]
    kind: Option<MediaType>,
    title: Option<Title>,
    cover_image: Option<CoverImage>,
    banner_image: Option<String>,
    format: Option<MediaFormat>,
    #[arguments(version: 2)]
    status: Option<MediaStatus>,
    season: Option<MediaSeason>,
    season_year: Option<i32>,
    episodes: Option<i32>,
    chapters: Option<i32>,
    mean_score: Option<i32>,
    genres: Option<Vec<Option<String>>>,
    is_adult: Option<bool>,
}

/// A schedule media is a card plus its next episode.
impl From<AiringMedia> for MediaCard {
    fn from(media: AiringMedia) -> Self {
        Self {
            id: media.id,
            id_mal: media.id_mal,
            kind: media.kind,
            title: media.title,
            cover_image: media.cover_image,
            banner_image: media.banner_image,
            format: media.format,
            status: media.status,
            season: media.season,
            season_year: media.season_year,
            episodes: media.episodes,
            chapters: media.chapters,
            mean_score: media.mean_score,
            genres: media.genres,
            is_adult: media.is_adult,
        }
    }
}

impl MediaSummary {
    /// The card of a schedule entry, dropping one with an invalid AniList
    /// id.
    fn from_airing(media: AiringMedia) -> Option<Self> {
        Self::from_card(media.into())
    }
}

#[derive(cynic::QueryVariables, Debug)]
struct PageVariables {
    page: i32,
    per_page: i32,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PageVariables")]
struct TrendingQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<TrendingPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page")]
struct TrendingPage {
    #[arguments(type: ANIME, sort: [TRENDING_DESC])]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PageVariables")]
struct PopularQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<PopularPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page")]
struct PopularPage {
    #[arguments(type: ANIME, sort: [POPULARITY_DESC])]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "UpcomingSeasonVariables")]
struct UpcomingSeasonQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<UpcomingSeasonPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page", variables = "UpcomingSeasonVariables")]
struct UpcomingSeasonPage {
    #[arguments(type: ANIME, season: $season, seasonYear: $season_year, sort: [POPULARITY_DESC])]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryVariables, Debug)]
struct UpcomingSeasonVariables {
    page: i32,
    per_page: i32,
    season: MediaSeason,
    season_year: i32,
}

impl UpcomingSeasonVariables {
    fn new() -> Self {
        let first = first_page();
        let (season, season_year) = upcoming_season(now());
        Self {
            page: first.page,
            per_page: first.per_page,
            season,
            season_year,
        }
    }
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PageVariables")]
struct TrendingMoviesQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<TrendingMoviesPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page")]
struct TrendingMoviesPage {
    #[arguments(type: ANIME, format: MOVIE, sort: [TRENDING_DESC])]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PageVariables")]
struct TrendingMangaQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<TrendingMangaPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page")]
struct TrendingMangaPage {
    #[arguments(type: MANGA, sort: [TRENDING_DESC])]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "TrendingByCountryVariables")]
struct TrendingByCountryQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<TrendingByCountryPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page", variables = "TrendingByCountryVariables")]
struct TrendingByCountryPage {
    #[arguments(type: ANIME, countryOfOrigin: $country, sort: [TRENDING_DESC])]
    media: Option<Vec<Option<MediaCard>>>,
}

#[derive(cynic::QueryVariables, Debug)]
struct TrendingByCountryVariables {
    page: i32,
    per_page: i32,
    country: CountryCode,
}

impl TrendingByCountryVariables {
    fn new(country: &str) -> Self {
        let first = first_page();
        Self {
            page: first.page,
            per_page: first.per_page,
            country: CountryCode(country.to_owned()),
        }
    }
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "PageVariables")]
struct AiringScheduleQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<SchedulePage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page")]
struct SchedulePage {
    #[arguments(notYetAired: true, sort: [TIME])]
    airing_schedules: Option<Vec<Option<ScheduleEntry>>>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "AiredRecentlyVariables")]
struct AiredRecentlyQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<AiredRecentlyPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page", variables = "AiredRecentlyVariables")]
struct AiredRecentlyPage {
    #[arguments(airingAt_lesser: $now, sort: [TIME_DESC])]
    airing_schedules: Option<Vec<Option<ScheduleEntry>>>,
}

#[derive(cynic::QueryVariables, Debug)]
struct AiredRecentlyVariables {
    page: i32,
    per_page: i32,
    now: i32,
}

impl AiredRecentlyVariables {
    fn new() -> Self {
        let first = first_page();
        Self {
            page: first.page,
            per_page: first.per_page,
            now: now(),
        }
    }
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "CollectionVariables")]
struct CollectionQuery {
    #[cynic(rename = "MediaListCollection")]
    #[arguments(userName: $user_name, type: ANIME)]
    media_list_collection: Option<Collection>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "MediaListCollection")]
struct Collection {
    lists: Option<Vec<Option<CollectionGroup>>>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "MediaListGroup")]
struct CollectionGroup {
    entries: Option<Vec<Option<CollectionEntry>>>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "MediaList")]
struct CollectionEntry {
    #[cynic(rename = "mediaId")]
    media_id: i32,
    status: Option<MediaListStatus>,
}

#[derive(cynic::QueryVariables, Debug)]
struct CollectionVariables {
    user_name: String,
}

impl CollectionVariables {
    fn new(user_name: &str) -> Self {
        Self {
            user_name: user_name.to_owned(),
        }
    }
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Query", variables = "RelationsVariables")]
struct RelationsQuery {
    #[cynic(rename = "Page")]
    #[arguments(page: $page, perPage: $per_page)]
    page: Option<RelationsPage>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Page", variables = "RelationsVariables")]
struct RelationsPage {
    #[arguments(id_in: $ids)]
    media: Option<Vec<Option<RelationMedia>>>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "Media")]
struct RelationMedia {
    relations: Option<MediaConnection>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
struct MediaConnection {
    edges: Option<Vec<Option<MediaRelationEdge>>>,
}

#[derive(cynic::QueryFragment, Debug, Default)]
#[cynic(graphql_type = "MediaEdge")]
struct MediaRelationEdge {
    #[arguments(version: 2)]
    relation_type: Option<MediaRelation>,
    node: Option<MediaCard>,
}

#[derive(cynic::QueryVariables, Debug)]
struct RelationsVariables {
    page: i32,
    per_page: i32,
    ids: Option<Vec<i32>>,
}

impl RelationsVariables {
    fn new(ids: Vec<i32>) -> Self {
        let first = first_page();
        Self {
            page: first.page,
            per_page: first.per_page,
            ids: Some(ids),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn civil_from_days_matches_known_dates() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(20_468), (2026, 1, 15));
        assert_eq!(civil_from_days(20_453), (2025, 12, 31));
        assert_eq!(civil_from_days(20_779), (2026, 11, 22));
    }

    #[test]
    fn upcoming_season_picks_the_season_after_the_current_one() {
        // Unix seconds for those dates, via `date -d`.
        assert_eq!(upcoming_season(1_795_305_600), (MediaSeason::Winter, 2027)); // 2026-11-22
        assert_eq!(upcoming_season(1_768_435_200), (MediaSeason::Spring, 2026)); // 2026-01-15
        assert_eq!(upcoming_season(1_775_001_600), (MediaSeason::Summer, 2026)); // 2026-04-01
        assert_eq!(upcoming_season(1_782_864_000), (MediaSeason::Fall, 2026)); // 2026-07-01
        assert_eq!(upcoming_season(1_767_139_200), (MediaSeason::Spring, 2026)); // 2025-12-31
    }
}
