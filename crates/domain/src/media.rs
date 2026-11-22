use serde::{Deserialize, Serialize};

/// Autumn's own identity for a media (ADR 0001).
///
/// Minted by `db` when the user first records something about a media, and
/// never changed while the media lives. Catalog data, caches and search
/// results never carry one: they name media by [`ExternalId`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct MediaId(i64);

impl MediaId {
    /// Wraps an id the database minted. Only `db` mints media ids; other
    /// crates get them from `db`.
    pub const fn new(id: i64) -> Self {
        Self(id)
    }

    /// The raw id, for storage.
    pub const fn get(self) -> i64 {
        self.0
    }
}

/// Whether a media is an anime or a manga. Both share the one [`MediaId`]
/// space; one media is one season or cour.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum MediaKind {
    /// An anime: series, film, OVA and the like.
    Anime,
    /// A manga, manhwa, manhua or light novel.
    Manga,
}

/// The id a provider gives a media. All providers count equally, and each
/// external id names at most one media (a cross-reference, ADR 0001).
///
/// Phase 1 uses only [`ExternalId::AniList`] and [`ExternalId::Mal`].
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ExternalId {
    /// An AniList media id.
    AniList(u32),
    /// A MyAnimeList id (AniList's `idMal`).
    Mal(u32),
    /// An AniDB anime id.
    AniDb(u32),
    /// A Kitsu id.
    Kitsu(u32),
    /// An item from a custom source extension.
    Custom {
        /// The custom source's id.
        source: String,
        /// The item's key within that source.
        key: String,
    },
}
