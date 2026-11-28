//! Batch suggestions on the Torrents tab (#227, #182 Q4).
//!
//! A **batch suggestion** strip above the Torrents tab list, only for
//! finished media with more than one episode. One batch search on the first
//! visit to the tab (within #181's budget and 60 s cache); the top
//! [`BATCH_SUGGESTION_COUNT`] full-episode batches by the effective
//! auto-select profile. Cards show group, range, resolution, codec, size,
//! seeders and indexer plus **Episodes ▾**, which fetches the file list on
//! demand. Picking a file streams it. No pinning.
//!
//! The view model here is pure and tested; searching and file listing go
//! through the engine (`render()` stays pure). Batch file pick reuses
//! [`crate::release_list::batch_files`] rather than a second picker.

use std::collections::BTreeMap;

use domain::TorrentFileRow;
use search::{EpisodeTarget, Indexer, Torrent};
use settings::AutoSelectProfile;

use crate::release_list::{self, BatchFile};

/// A suggestion's file-list state; reuses the release list's batch state.
pub use crate::release_list::BatchState;

/// How many batch suggestions are shown.
pub const BATCH_SUGGESTION_COUNT: usize = 3;

/// The strip's runtime state (#227).
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct BatchSuggestions {
    /// One search has been kicked off on first visit; no re-search.
    pub started: bool,
    /// The batch search is in flight.
    pub loading: bool,
    /// Raw results, kept for re-ranking when the profile changes.
    pub raw: Vec<Torrent>,
    /// Top suggestions by the effective profile.
    pub suggestions: Vec<BatchSuggestion>,
    /// The last search error.
    pub error: Option<String>,
    /// Expanded infohash (one at a time).
    pub expanded: Option<String>,
    /// Per-infohash on-demand file-list state.
    pub batches: BTreeMap<String, BatchState>,
}

/// One batch suggestion card.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BatchSuggestion {
    /// The underlying indexer result, for streaming and file listing.
    pub torrent: Torrent,
    /// Lowercase v1 infohash, the card key.
    pub infohash: String,
    /// Release name, as written on the indexer.
    pub name: String,
    /// Release group, when known.
    pub group: Option<String>,
    /// Episode range covered, e.g. `1–12`.
    pub range: String,
    /// `1080p` etc, when known.
    pub resolution: Option<u32>,
    /// Video codec label, when known.
    pub codec: Option<String>,
    /// Total size in bytes, when known.
    pub size: Option<u64>,
    /// Seeders, when known.
    pub seeders: Option<u32>,
    /// Origin indexer.
    pub indexer: Indexer,
}

/// Whether the strip shows: finished media with more than one episode
/// (#182 Q4).
#[expect(clippy::fn_params_excessive_bools, reason = "one flag: finished")]
#[must_use]
pub fn should_show(finished: bool, total_episodes: u32) -> bool {
    finished && total_episodes > 1
}

/// The match target for ranking batch suggestions and preselecting a file:
/// season `season`, first episode.
#[must_use]
pub const fn suggestion_target(season: u32) -> EpisodeTarget {
    EpisodeTarget::new(season, 1)
}

/// Whether `torrent` is a batch covering every episode of a media with
/// `total_episodes`. A range must cover `1..=total_episodes`; a `Batch` /
/// `Complete` flag with no contradicting range counts; partial batches and
/// non-episodes do not.
#[must_use]
pub fn covers_every_episode(torrent: &Torrent, total_episodes: u32) -> bool {
    if total_episodes <= 1 {
        return false;
    }
    let mut release = release_parser::parse(&torrent.name);
    release.batch |= torrent.is_batch;
    if release.kind != release_parser::EpisodeKind::Episode || !release.batch {
        return false;
    }
    if release.episode == Some(0) {
        return false;
    }
    match (release.episode, release.last_episode) {
        // An explicit range must start at 1 and reach the last episode.
        (Some(first), Some(last)) => first == 1 && first <= last && last >= total_episodes,
        // `Batch` / `Complete`, no episode number at all.
        (None, _) => true,
        // A single numbered episode can't prove full coverage.
        (Some(_), None) => false,
    }
}

/// Top `count` full-episode batches by `profile`, best first (#182 Q4).
/// `season` is this media's season `M`; `total_episodes` its episode count.
#[must_use]
pub fn batch_suggestions(
    results: Vec<Torrent>,
    season: u32,
    total_episodes: u32,
    profile: &AutoSelectProfile,
    count: usize,
) -> Vec<BatchSuggestion> {
    let full_batches: Vec<Torrent> = results
        .into_iter()
        .filter(|torrent| covers_every_episode(torrent, total_episodes))
        .collect();
    let target = suggestion_target(season);
    search::rank(full_batches, &target, profile)
        .into_iter()
        .take(count)
        .map(|torrent| suggestion(torrent, total_episodes))
        .collect()
}

/// Builds one card from a torrent.
fn suggestion(torrent: Torrent, total_episodes: u32) -> BatchSuggestion {
    let release = release_parser::parse(&torrent.name);
    let range = match (release.episode, release.last_episode) {
        (Some(first), Some(last)) => format!("{first}–{last}"),
        _ => format!("1–{total_episodes}"),
    };
    BatchSuggestion {
        infohash: torrent.infohash.to_ascii_lowercase(),
        name: torrent.name.clone(),
        group: release.group,
        resolution: release.resolution,
        codec: release.video_codec.map(release_list::codec_label),
        size: torrent.size,
        seeders: torrent.seeders,
        indexer: torrent.indexer.clone(),
        range,
        torrent,
    }
}

/// **Episodes ▾**: builds the ordered batch file list from an on-demand
/// fetch, preselecting episode 1; reuses the release list's batch file
/// pick. `AnimeTosho` file lists when present, else the indexer's torrent
/// file. NCOP/NCED/sample/extra are dimmed and can't be played.
#[must_use]
pub fn suggestion_files(rows: Vec<TorrentFileRow>, season: u32) -> Vec<BatchFile> {
    release_list::batch_files(rows, &suggestion_target(season))
}

/// The episode a picked file plays as: its parsed episode, else `requested`.
pub use release_list::file_episode;

/// The torrent file list when a torrent already carries it (`AnimeTosho`),
/// else empty.
pub use release_list::indexer_files;

#[cfg(test)]
mod tests {
    use super::*;

    use domain::TorrentFileRow;
    use search::Indexer;

    fn batch_torrent(name: &str, seeders: u32, size: u64) -> Torrent {
        let mut t = Torrent::new(Indexer::Nyaa, "a".repeat(40), name);
        t.seeders = Some(seeders);
        t.size = Some(size);
        t
    }

    fn full_batch(name: &str, seeders: u32, size: u64) -> Torrent {
        batch_torrent(name, seeders, size)
    }

    #[test]
    fn shows_only_for_finished_media_with_more_than_one_episode() {
        assert!(should_show(true, 12));
        assert!(should_show(true, 2));
        assert!(!should_show(false, 12), "not finished");
        assert!(!should_show(true, 1), "single episode");
        assert!(!should_show(true, 0), "no episodes");
        assert!(!should_show(false, 1));
    }

    #[test]
    fn full_range_batch_covers_every_episode() {
        let t = full_batch("[Grp] Show (01-12) [1080p]", 10, 1);
        assert!(covers_every_episode(&t, 12));
        let t = full_batch("[Grp] Show 01-12 [1080p]", 10, 1);
        assert!(covers_every_episode(&t, 12));
    }

    #[test]
    fn range_must_start_at_1_and_reach_the_last_episode() {
        let partial = full_batch("[Grp] Show (01-06) [1080p]", 10, 1);
        assert!(!covers_every_episode(&partial, 12), "partial batch");
        let short = full_batch("[Grp] Show (01-11) [1080p]", 10, 1);
        assert!(!covers_every_episode(&short, 12), "misses last episode");
        let late = full_batch("[Grp] Show (02-12) [1080p]", 10, 1);
        assert!(!covers_every_episode(&late, 12), "misses first episode");
    }

    #[test]
    fn flagged_batch_with_no_contradicting_range_covers_every_episode() {
        let t = full_batch("[Grp] Show Batch [1080p]", 10, 1);
        assert!(covers_every_episode(&t, 12));
        let mut t = full_batch("[Grp] Show [1080p]", 10, 1);
        t.is_batch = true;
        assert!(covers_every_episode(&t, 12));
    }

    #[test]
    fn non_batches_and_non_episodes_do_not_cover_every_episode() {
        let single = full_batch("[Grp] Show - 05 [1080p]", 10, 1);
        assert!(!covers_every_episode(&single, 12));
        let nc = full_batch("[Grp] Show NCOP [1080p]", 10, 1);
        assert!(!covers_every_episode(&nc, 12));
    }

    #[test]
    fn top_3_by_effective_profile() {
        // Four full batches; only three are shown, best first.
        let results = vec![
            full_batch("[Grp] Show (01-12) [720p]", 999, 1_000_000),
            full_batch("[Grp] Show (01-12) [1080p]", 10, 1_000_000),
            full_batch("[Grp] Show (01-12) [1080p]", 50, 1_000_000),
            full_batch("[Grp] Show (01-12) [1080p]", 500, 1_000_000),
        ];
        let got = batch_suggestions(
            results,
            1,
            12,
            &AutoSelectProfile::default(),
            BATCH_SUGGESTION_COUNT,
        );
        assert_eq!(got.len(), BATCH_SUGGESTION_COUNT, "top 3");
        // Resolution outranks seeders; then seeders.
        assert_eq!(got.first().and_then(|s| s.resolution), Some(1080));
        assert_eq!(got.first().and_then(|s| s.seeders), Some(500));
        assert_eq!(got.get(1).and_then(|s| s.seeders), Some(50));
        assert_eq!(got.get(2).and_then(|s| s.seeders), Some(10));
    }

    #[test]
    fn partial_batches_and_singles_are_excluded() {
        let results = vec![
            full_batch("[Grp] Show - 05 [1080p]", 999, 1),
            full_batch("[Grp] Show (01-06) [1080p]", 999, 1),
            full_batch("[Grp] Show (01-12) [1080p]", 10, 1),
        ];
        let got = batch_suggestions(
            results,
            1,
            12,
            &AutoSelectProfile::default(),
            BATCH_SUGGESTION_COUNT,
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got.first().map(|s| s.range.as_str()), Some("1–12"));
    }

    #[test]
    fn no_full_batch_means_no_suggestions() {
        let results = vec![full_batch("[Grp] Show - 01 [1080p]", 999, 1)];
        let got = batch_suggestions(
            results,
            1,
            12,
            &AutoSelectProfile::default(),
            BATCH_SUGGESTION_COUNT,
        );
        assert_eq!(got, Vec::new());
    }

    #[test]
    fn card_fields_group_range_resolution_codec_size_seeders_indexer() {
        let mut t = full_batch("[Grp] Show (01-12) [1080p] HEVC", 42, 1234);
        t.size = Some(1234);
        let got = batch_suggestions(
            vec![t],
            1,
            12,
            &AutoSelectProfile::default(),
            BATCH_SUGGESTION_COUNT,
        );
        let card = got.first().cloned().unwrap();
        assert_eq!(card.group.as_deref(), Some("Grp"));
        assert_eq!(card.range, "1–12");
        assert_eq!(card.resolution, Some(1080));
        assert_eq!(card.codec.as_deref(), Some("HEVC"));
        assert_eq!(card.size, Some(1234));
        assert_eq!(card.seeders, Some(42));
        assert_eq!(card.indexer, Indexer::Nyaa);
    }

    #[test]
    fn on_demand_file_list_reuses_batch_files_with_episode_1_preselected() {
        let rows = vec![
            TorrentFileRow::new(0, "Show - 01.mkv", 100, false),
            TorrentFileRow::new(1, "Show - 02.mkv", 100, false),
            TorrentFileRow::new(2, "NCOP.mkv", 50, false),
            TorrentFileRow::new(3, "Show - 12.mkv", 100, false),
        ];
        let files = suggestion_files(rows, 1);
        assert_eq!(files.len(), 4, "all files listed");
        assert!(
            files.first().is_some_and(|f| f.preselected),
            "episode 1 preselected"
        );
        assert!(files.first().is_some_and(|f| f.playable));
        assert!(!files.get(2).is_some_and(|f| f.playable), "NCOP dimmed");
        // Other episode files play as their own episode.
        assert_eq!(files.get(3).and_then(|f| f.episode), Some(12));
        assert_eq!(file_episode("Show - 12.mkv", 1), 12);
        assert_eq!(file_episode("NCOP.mkv", 1), 1);
    }
}
