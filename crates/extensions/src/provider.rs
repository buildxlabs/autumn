//! The torrent-provider contract (#268): what `search(request)` receives and
//! returns, as plain Rust types.
//!
//! Luau tables are one encoding of these types; declarative sources (ADR 0016)
//! produce them natively. Validating rows and mapping them to a `Torrent` is
//! `search`'s job (ADR 0011), so [`ProviderTorrent`] stays raw.

/// One search a torrent provider is asked to run.
///
/// Encoded for Luau as a fresh `snake_case` table: absent fields are `nil`, and
/// [`EpisodeQuery`]'s fields appear only for a structured search, so a
/// provider tells the two apart with `if request.episode then`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderRequest {
    /// Always set: the user's text, or the first title of a structured
    /// search.
    pub keyword: String,
    /// The titles, season and episode of a structured search, or `None` for a
    /// keyword search.
    pub episode: Option<EpisodeQuery>,
    /// The episode number counted from the first season, when known.
    pub absolute_episode: Option<u32>,
    /// The AniDB anime id, when known.
    pub anidb_aid: Option<u32>,
    /// The AniDB episode id, when known.
    pub anidb_eid: Option<u32>,
}

impl ProviderRequest {
    /// A keyword search with no episode or ids.
    #[must_use]
    pub fn keyword(keyword: impl Into<String>) -> Self {
        Self {
            keyword: keyword.into(),
            episode: None,
            absolute_episode: None,
            anidb_aid: None,
            anidb_eid: None,
        }
    }
}

/// The fields a structured search always sets together (#268 Q2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpisodeQuery {
    /// Ordered title variants; the first equals the request's keyword.
    pub titles: Vec<String>,
    /// The season, 1 or more.
    pub season: u32,
    /// The episode within the season; 0 means specials.
    pub episode: u32,
}

/// One row a provider returned, before `search` validates it.
///
/// Every field is optional. A Luau value of the wrong type (a string where a
/// number belongs) decodes as `None`, with a `debug` line naming its path;
/// ranges, integer checks, hash formats and URL rules are `search`'s. Unknown
/// row fields are ignored. Luau numbers are doubles, so numeric fields keep
/// them as `f64` for `search` to check.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ProviderTorrent {
    /// The release name.
    pub name: Option<String>,
    /// A v1 infohash, as 40 hex or 32 base32 characters.
    pub infohash: Option<String>,
    /// A `magnet:` URI.
    pub magnet_url: Option<String>,
    /// A link to the `.torrent` file.
    pub torrent_url: Option<String>,
    /// The total size in bytes.
    pub size: Option<f64>,
    /// Seeders reported by the site.
    pub seeders: Option<f64>,
    /// Leechers reported by the site.
    pub leechers: Option<f64>,
    /// Completed downloads reported by the site.
    pub downloads: Option<f64>,
    /// When the release was published, in Unix seconds.
    pub published: Option<f64>,
    /// The site's hint that this is a batch.
    pub is_batch: Option<bool>,
    /// The AniDB anime id.
    pub anidb_aid: Option<f64>,
    /// The AniDB episode id.
    pub anidb_eid: Option<f64>,
    /// The number of files in the torrent.
    pub num_files: Option<f64>,
    /// File names in the torrent; non-string entries are dropped.
    pub files: Option<Vec<String>>,
}
