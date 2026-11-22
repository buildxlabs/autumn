//! Query fragments shared by several queries, and their conversion into the
//! public types.

use domain::{FuzzyDate, MediaKind};

use crate::{MediaFormat, MediaSeason, MediaStatus, MediaSummary, MediaTitle, schema};

/// The card fields of a media.
#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "Media")]
pub(crate) struct MediaCard {
    pub(crate) id: i32,
    pub(crate) id_mal: Option<i32>,
    #[cynic(rename = "type")]
    pub(crate) kind: Option<MediaType>,
    pub(crate) title: Option<Title>,
    pub(crate) cover_image: Option<CoverImage>,
    pub(crate) banner_image: Option<String>,
    pub(crate) format: Option<MediaFormat>,
    #[arguments(version: 2)]
    pub(crate) status: Option<MediaStatus>,
    pub(crate) season: Option<MediaSeason>,
    pub(crate) season_year: Option<i32>,
    pub(crate) episodes: Option<i32>,
    pub(crate) chapters: Option<i32>,
    pub(crate) mean_score: Option<i32>,
    pub(crate) genres: Option<Vec<Option<String>>>,
    pub(crate) is_adult: Option<bool>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaTitle")]
pub(crate) struct Title {
    romaji: Option<String>,
    english: Option<String>,
    native: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "MediaCoverImage")]
pub(crate) struct CoverImage {
    extra_large: Option<String>,
    large: Option<String>,
    medium: Option<String>,
}

#[derive(cynic::QueryFragment, Debug)]
#[cynic(graphql_type = "FuzzyDate")]
pub(crate) struct Date {
    year: Option<i32>,
    month: Option<i32>,
    day: Option<i32>,
}

/// AniList's `MediaType`, mapped onto [`MediaKind`].
#[derive(cynic::Enum, Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MediaType {
    Anime,
    Manga,
}

impl MediaType {
    /// The AniList type for a kind, or `None` for a kind AniList lacks.
    pub(crate) fn from_kind(kind: MediaKind) -> Option<Self> {
        match kind {
            MediaKind::Anime => Some(Self::Anime),
            MediaKind::Manga => Some(Self::Manga),
            _ => None,
        }
    }
}

impl From<MediaType> for MediaKind {
    fn from(kind: MediaType) -> Self {
        match kind {
            MediaType::Anime => Self::Anime,
            MediaType::Manga => Self::Manga,
        }
    }
}

impl From<MediaCard> for MediaSummary {
    fn from(card: MediaCard) -> Self {
        Self {
            anilist_id: count(card.id).unwrap_or_default(),
            mal_id: card.id_mal.and_then(count),
            kind: card.kind.map(MediaKind::from),
            title: card.title.map_or_default(MediaTitle::from),
            poster: card.cover_image.and_then(CoverImage::poster),
            banner: card.banner_image,
            format: card.format,
            status: card.status,
            season: card.season,
            season_year: card.season_year.and_then(|year| u16::try_from(year).ok()),
            episodes: card.episodes.and_then(count),
            chapters: card.chapters.and_then(count),
            mean_score: card.mean_score.and_then(|score| u8::try_from(score).ok()),
            genres: card.genres.into_iter().flatten().flatten().collect(),
            is_adult: card.is_adult.unwrap_or_default(),
        }
    }
}

impl From<Title> for MediaTitle {
    fn from(title: Title) -> Self {
        Self {
            romaji: title.romaji,
            english: title.english,
            native: title.native,
        }
    }
}

impl CoverImage {
    /// The largest cover AniList has: `extraLarge`, then `large`, then
    /// `medium` (decision #20).
    fn poster(self) -> Option<String> {
        self.extra_large.or(self.large).or(self.medium)
    }
}

impl Date {
    /// The date's valid parts. An impossible month or day keeps just the
    /// year, which is always valid.
    pub(crate) fn into_fuzzy(self) -> FuzzyDate {
        let year = self.year.and_then(|year| u16::try_from(year).ok());
        let month = self.month.and_then(|month| u8::try_from(month).ok());
        let day = self.day.and_then(|day| u8::try_from(day).ok());
        FuzzyDate::new(year, month, day)
            .or_else(|_| FuzzyDate::new(year, None, None))
            .unwrap_or_default()
    }
}

/// A GraphQL `Int` that counts or identifies something, so is never negative.
pub(crate) fn count(value: i32) -> Option<u32> {
    u32::try_from(value).ok()
}

/// AniList's `CountryCode` scalar: an ISO 3166-1 alpha-2 code.
#[derive(cynic::Scalar, Debug, Clone)]
pub(crate) struct CountryCode(pub(crate) String);
