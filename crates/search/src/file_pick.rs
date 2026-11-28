//! The file-level episode check and file pick inside one torrent (#181 Q6, Q11).
//!
//! A release-level match is only a promise: auto-select confirms it by
//! finding the episode's own video file. Creditless openings and endings,
//! specials and other extras never match (ADR 0007), and samples or files
//! under an extras folder are skipped.

use std::cmp::Reverse;

use release_parser::{EpisodeKind, parse};

use crate::{EpisodeTarget, matching::matches};

/// Extensions of files a player can open as an episode.
const VIDEO_EXTENSIONS: &[&str] = &[
    "mkv", "mp4", "m4v", "avi", "webm", "mov", "ts", "m2ts", "wmv", "flv", "ogm",
];

/// Path words that mark a file as not an episode, matched case-insensitively.
const SKIPPED_WORDS: &[&str] = &[
    "sample",
    "samples",
    "extra",
    "extras",
    "bonus",
    "specials",
    "creditless",
    "featurettes",
    "trailers",
];

/// The index of the file holding `target`, from `(index, path)` pairs.
///
/// Each path is a `/`-separated path inside the torrent; only its file name is
/// parsed for the episode. Among several copies of the episode, the highest
/// version wins, then the first listed. A torrent whose only video has no
/// episode number yields that video, since the release name already matched.
#[must_use]
pub fn pick_file<'a>(
    files: impl IntoIterator<Item = (usize, &'a str)>,
    target: &EpisodeTarget,
) -> Option<usize> {
    let mut videos = 0_usize;
    let mut lone_unnumbered = None;
    let mut best: Option<(u32, Reverse<usize>)> = None;
    for (index, path) in files {
        let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
        let release = parse(name);
        let is_video = release
            .extension
            .as_deref()
            .is_some_and(|extension| VIDEO_EXTENSIONS.contains(&extension));
        if !is_video || is_skipped(path) {
            continue;
        }
        videos += 1;
        if matches(&release, target) && release.last_episode.is_none() && release.episode.is_some()
        {
            let key = (release.version.unwrap_or(1), Reverse(index));
            best = best.max(Some(key));
        } else if release.episode.is_none() && release.kind == EpisodeKind::Episode {
            lone_unnumbered = Some(index);
        }
    }
    best.map(|(_, Reverse(index))| index)
        .or_else(|| lone_unnumbered.filter(|_| videos == 1))
}

/// Whether `path`'s extension is a video a player can open as an episode
/// (and not padding): `.mkv`, `.mp4`, `.m4v`, `.avi`, `.webm`, `.mov`, `.ts`,
/// `.m2ts`, `.wmv`, `.flv`, `.ogm`. Used to reject a non-video `.torrent`.
#[must_use]
pub fn is_video_path(path: &str) -> bool {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    release_parser::parse(name)
        .extension
        .as_deref()
        .is_some_and(|extension| VIDEO_EXTENSIONS.contains(&extension))
}

fn is_skipped(path: &str) -> bool {
    path.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .any(|word| SKIPPED_WORDS.contains(&word))
}
