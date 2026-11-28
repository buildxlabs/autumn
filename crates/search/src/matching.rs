//! Episode matching on `release-parser` output (ADR 0007).
//!
//! Matching is pure: no indexer fields beyond a batch flag reach here, and no
//! title similarity is recomputed. Query construction and `AniDB` `aid` carry
//! title relevance; this module only answers whether a release contains the
//! requested episode of the media's season.

use release_parser::{EpisodeKind, Release, parse};

use crate::result::Torrent;

/// The episode being looked for, in both numbering schemes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct EpisodeTarget {
    /// This media's season `M`: `AniZip` `season_number`, a season parsed from
    /// the title, or 1. `0` is specials and never requested here.
    pub season: u32,
    /// The requested episode `N`, counted within this media.
    pub episode: u32,
    /// `AniZip` `absolute_number` when it differs from [`Self::episode`].
    pub absolute_episode: Option<u32>,
}

impl EpisodeTarget {
    /// Target an episode of a media without absolute numbering.
    #[must_use]
    pub const fn new(season: u32, episode: u32) -> Self {
        Self {
            season,
            episode,
            absolute_episode: None,
        }
    }

    /// Add the absolute number when `AniZip` gives one different from `episode`.
    #[must_use]
    pub const fn with_absolute_episode(mut self, absolute: u32) -> Self {
        self.absolute_episode = Some(absolute);
        self
    }
}

/// Whether a parsed release contains the requested episode (ADR 0007, #181 Q5–Q7).
///
/// Versions (`NNvK`) are ignored for matching, specials and other non-episode
/// kinds never match, a known season mismatch rejects, and a batch matches
/// when its range covers `N` or it is flagged with no contradicting range.
#[must_use]
pub fn matches(release: &Release, target: &EpisodeTarget) -> bool {
    if release.kind != EpisodeKind::Episode || !season_covers(release, target.season) {
        return false;
    }
    // Episode zero is the specials marker (`S2 - 00`), even without `S00`.
    if release.episode == Some(0) {
        return false;
    }
    if let Some(last) = release.last_episode {
        let Some(first) = release.episode else {
            return false;
        };
        if first > last {
            return false;
        }
        return (first..=last).contains(&target.episode)
            || target
                .absolute_episode
                .is_some_and(|absolute| (first..=last).contains(&absolute));
    }
    match release.episode {
        Some(number) => number == target.episode || target.absolute_episode == Some(number),
        // `Batch` / `Complete` / a season range with no episode number.
        None => release.batch,
    }
}

/// Pull the matchable `release-parser` titles out of release names (#209
/// Q2): parse each name, keep non-blank `Release::title`s, dedupe
/// case-sensitively, preserve order.
#[must_use]
pub fn match_titles(names: &[String]) -> Vec<String> {
    let mut titles = Vec::new();
    for name in names {
        if let Some(title) = parse(name).title {
            let title = title.trim().to_owned();
            if !title.is_empty() && !titles.contains(&title) {
                titles.push(title);
            }
        }
    }
    titles
}

/// The first `Release::episode` number parsed from `names`, in order (#209
/// Q3): the pre-selected episode for match review.
#[must_use]
pub fn episode_suggestion(names: &[String]) -> Option<u32> {
    names.iter().find_map(|name| parse(name).episode)
}

/// Match one indexer result by parsing its release name (Q5–Q7).
///
/// An indexer's batch flag counts as the batch marker when the name does not
/// state one; the rest of the decision stays on `release-parser` output.
#[must_use]
pub fn matches_torrent(torrent: &Torrent, target: &EpisodeTarget) -> bool {
    let mut release = parse(&torrent.name);
    release.batch |= torrent.is_batch;
    matches(&release, target)
}

/// A release with no season marker is accepted; a known one must be `M` or a
/// season range covering `M`.
fn season_covers(release: &Release, season: u32) -> bool {
    match release.season {
        None => true,
        Some(0) => false,
        Some(first) => {
            first == season
                || release
                    .last_season
                    .is_some_and(|last| first <= season && season <= last)
        }
    }
}
