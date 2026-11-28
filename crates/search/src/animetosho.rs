//! `AnimeTosho` JSON feed result parsing (ADR 0008).
//!
//! The live successor at `feed.animetosho.net/json` returns an array of
//! results with `AniDB` ids and a file count. The file list itself needs a
//! separate `?show=torrent&id=` request, reserved for shortlisted results.

use serde::Deserialize;

use crate::{Error, Indexer, Torrent, util};

/// The fields of one search result that Autumn consumes.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct SearchItem {
    title: String,
    info_hash: Option<String>,
    info_hash_v2: Option<String>,
    magnet_uri: Option<String>,
    torrent_url: Option<String>,
    total_size: Option<u64>,
    seeders: Option<u32>,
    leechers: Option<u32>,
    torrent_downloaded_count: Option<u32>,
    is_seadex: Option<bool>,
    anidb_aid: Option<u32>,
    anidb_eid: Option<u32>,
    num_files: Option<u32>,
    timestamp: Option<u64>,
}

/// Parse one page of `AnimeTosho` JSON results, in feed order.
///
/// Results without any infohash are skipped: the infohash is the dedupe key.
/// A v2-only hash is kept so those results still have a stable identity.
///
/// # Errors
///
/// Returns [`Error::Json`] if the body is not the expected JSON array.
pub fn parse_search(json: &str) -> Result<Vec<Torrent>, Error> {
    let items: Vec<SearchItem> =
        serde_json::from_str(json).map_err(|error| Error::Json(error.to_string()))?;
    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let Some(hash) = item.info_hash.or(item.info_hash_v2) else {
            continue;
        };
        let hash = hash.to_ascii_lowercase();
        let mut torrent = Torrent::new(Indexer::AnimeTosho, hash.clone(), item.title);
        torrent.torrent_url = item.torrent_url;
        torrent.magnet_url = item.magnet_uri.or_else(|| Some(util::magnet_for(&hash)));
        torrent.size = item.total_size;
        torrent.seeders = item.seeders;
        torrent.leechers = item.leechers;
        torrent.downloads = item.torrent_downloaded_count;
        torrent.is_seadex = item.is_seadex.unwrap_or(false);
        torrent.anidb_aid = item.anidb_aid;
        torrent.anidb_eid = item.anidb_eid;
        torrent.num_files = item.num_files;
        torrent.published = item.timestamp;
        results.push(torrent);
    }
    Ok(results)
}
