//! Plain result types. They name media by AniList id and carry no `cynic`
//! query types, so callers never depend on the query shapes.

use domain::{ExternalId, FuzzyDate, MediaKind};
use serde::{Deserialize, Serialize};

use crate::schema;

/// A media as a card shows it: in search results, relations and
/// recommendations.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaSummary {
    /// AniList's media id.
    pub anilist_id: u32,
    /// MyAnimeList's id (`idMal`), when AniList knows it.
    pub mal_id: Option<u32>,
    /// Anime or manga.
    pub kind: Option<MediaKind>,
    /// The titles in each language AniList has.
    pub title: MediaTitle,
    /// The poster: `coverImage.extraLarge`, falling back to `large`, then
    /// `medium`.
    pub poster: Option<String>,
    /// The banner (`bannerImage`), which many media lack.
    pub banner: Option<String>,
    /// How the media was released.
    pub format: Option<MediaFormat>,
    /// Whether it is still releasing.
    pub status: Option<MediaStatus>,
    /// The season it first aired in.
    pub season: Option<MediaSeason>,
    /// The year of that season.
    pub season_year: Option<u16>,
    /// The episode count when complete (anime).
    pub episodes: Option<u32>,
    /// The chapter count when complete (manga).
    pub chapters: Option<u32>,
    /// The mean of users' scores, 0–100.
    pub mean_score: Option<u8>,
    /// Genre names.
    pub genres: Vec<String>,
    /// Whether the media is for adults only.
    pub is_adult: bool,
}

impl MediaSummary {
    /// The external ids this media is known by, AniList first, for looking
    /// up or minting its `MediaId` through `db`.
    #[must_use]
    pub fn external_ids(&self) -> Vec<ExternalId> {
        std::iter::once(ExternalId::AniList(self.anilist_id))
            .chain(self.mal_id.map(ExternalId::Mal))
            .collect()
    }
}

/// Everything the Anime Entry page shows about one media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaDetail {
    /// The card fields: ids, titles, poster, banner and the metadata strip.
    pub summary: MediaSummary,
    /// The synopsis, as AniList's HTML.
    pub description: Option<String>,
    /// The first release date.
    pub start_date: FuzzyDate,
    /// The last release date.
    pub end_date: FuzzyDate,
    /// Minutes per episode.
    pub duration: Option<u32>,
    /// Names of the main studios.
    pub studios: Vec<String>,
    /// How many users have the media on a list.
    pub popularity: Option<u32>,
    /// Main characters first, each with their Japanese voice actors.
    pub characters: Vec<Character>,
    /// Prequels, sequels, adaptations and other related media.
    pub relations: Vec<Relation>,
    /// Media users recommend alongside this one, best rated first.
    pub recommendations: Vec<MediaSummary>,
}

/// The titles of a media. Any of them may be missing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MediaTitle {
    /// The romanized native title.
    pub romaji: Option<String>,
    /// The official English title.
    pub english: Option<String>,
    /// The title in its native language.
    pub native: Option<String>,
}

/// A character in a media.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Character {
    /// AniList's character id.
    pub anilist_id: u32,
    /// The character's full name.
    pub name: Option<String>,
    /// A portrait: `image.large`, falling back to `medium`.
    pub image: Option<String>,
    /// How central the character is to this media.
    pub role: Option<CharacterRole>,
    /// The character's Japanese voice actors.
    pub voice_actors: Vec<VoiceActor>,
}

/// A voice actor for a character.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VoiceActor {
    /// AniList's staff id.
    pub anilist_id: u32,
    /// The actor's full name.
    pub name: Option<String>,
    /// A portrait: `image.large`, falling back to `medium`.
    pub image: Option<String>,
}

/// A media related to another, and how.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Relation {
    /// How the related media relates to the one it was fetched with.
    pub relation: Option<MediaRelation>,
    /// The related media.
    pub media: MediaSummary,
}

/// How a media was released (AniList's `MediaFormat`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum MediaFormat {
    /// A TV series.
    Tv,
    /// A TV series with episodes under 15 minutes.
    TvShort,
    /// A film.
    Movie,
    /// A special episode.
    Special,
    /// An original video animation.
    Ova,
    /// An original net animation.
    Ona,
    /// A music video.
    Music,
    /// A manga, manhwa or manhua.
    Manga,
    /// A light novel.
    Novel,
    /// A one-shot manga.
    OneShot,
    /// A format this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Other(String),
}

/// Whether a media is still releasing (AniList's `MediaStatus`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum MediaStatus {
    /// Finished releasing.
    Finished,
    /// Currently releasing.
    Releasing,
    /// Announced but not released yet.
    NotYetReleased,
    /// Ended before it finished.
    Cancelled,
    /// Paused, to be resumed later.
    Hiatus,
    /// A status this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Other(String),
}

/// The season a media first aired in (AniList's `MediaSeason`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum MediaSeason {
    /// December to February.
    Winter,
    /// March to May.
    Spring,
    /// June to August.
    Summer,
    /// September to November.
    Fall,
    /// A season this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Other(String),
}

/// How one media relates to another (AniList's `MediaRelation`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum MediaRelation {
    /// An adaptation of this media into another format.
    Adaptation,
    /// Released before this media.
    Prequel,
    /// Released after this media.
    Sequel,
    /// The media this one is a side story of.
    Parent,
    /// A side story of this media.
    SideStory,
    /// Shares characters.
    Character,
    /// A summary of this media.
    Summary,
    /// An alternative version of the same story.
    Alternative,
    /// A spin-off.
    SpinOff,
    /// Related in another way.
    Other,
    /// The source material.
    Source,
    /// A compilation of this media.
    Compilation,
    /// Contained in this media.
    Contains,
    /// Set in the same universe.
    SameUniverse,
    /// A relation this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Unknown(String),
}

/// How central a character is to a media (AniList's `CharacterRole`).
#[derive(cynic::Enum, Debug, Clone, PartialEq, Eq, Hash)]
#[cynic(non_exhaustive)]
#[non_exhaustive]
pub enum CharacterRole {
    /// A main character.
    Main,
    /// A supporting character.
    Supporting,
    /// A background character.
    Background,
    /// A role this build does not know, as AniList spells it.
    #[cynic(fallback)]
    Other(String),
}
