//! The indexer-neutral [`Torrent`] result, plain data shared with `debrid`.

/// Which indexer produced a [`Torrent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Indexer {
    /// `Nyaa`, by keyword only.
    Nyaa,
    /// `AnimeTosho`'s live `feed.animetosho.net` JSON API.
    AnimeTosho,
}

impl Indexer {
    /// A short name for logs and UI labels.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Nyaa => "Nyaa",
            Self::AnimeTosho => "AnimeTosho",
        }
    }
}

/// One torrent-indexer result (glossary **Torrent**).
///
/// Only data: no client handle, no promises about file-level verification.
/// Indexers that do not report a field leave it `None` or empty.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
#[expect(
    clippy::struct_excessive_bools,
    reason = "indexer flags are independent recorded fields; ranking reads them in #218"
)]
pub struct Torrent {
    /// The indexer this result came from.
    pub indexer: Indexer,
    /// Lowercase v1 infohash, also the dedupe key.
    pub infohash: String,
    /// Release name, as written on the indexer.
    pub name: String,
    /// The `.torrent` download link, if offered.
    pub torrent_url: Option<String>,
    /// The magnet link, if offered or derivable from the infohash.
    pub magnet_url: Option<String>,
    /// Total size in bytes.
    pub size: Option<u64>,
    /// Seeders, when the indexer reports them.
    pub seeders: Option<u32>,
    /// Leechers, when the indexer reports them.
    pub leechers: Option<u32>,
    /// Completed downloads, when the indexer reports them.
    pub downloads: Option<u32>,
    /// `Nyaa`'s trusted flag.
    pub trusted: bool,
    /// `Nyaa`'s remake flag.
    pub remake: bool,
    /// `AnimeTosho`'s batch flag, when known from torrent details.
    pub is_batch: bool,
    /// `AnimeTosho`'s `is_seadex` flag.
    pub is_seadex: bool,
    /// `AniDB` anime id, when the indexer reports one.
    pub anidb_aid: Option<u32>,
    /// `AniDB` episode id, when the indexer reports one.
    pub anidb_eid: Option<u32>,
    /// Number of files, when the indexer reports it.
    pub num_files: Option<u32>,
    /// File names, when a file list was fetched for this result.
    pub files: Vec<String>,
}

impl Torrent {
    /// A result with only its required fields; indexer-specific fields default.
    #[must_use]
    pub fn new(indexer: Indexer, infohash: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            indexer,
            infohash: infohash.into(),
            name: name.into(),
            torrent_url: None,
            magnet_url: None,
            size: None,
            seeders: None,
            leechers: None,
            downloads: None,
            trusted: false,
            remake: false,
            is_batch: false,
            is_seadex: false,
            anidb_aid: None,
            anidb_eid: None,
            num_files: None,
            files: Vec::new(),
        }
    }
}
