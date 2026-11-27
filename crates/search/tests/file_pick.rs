//! File-level episode check and file pick inside a torrent (#181 Q6, Q11).

use search::{EpisodeTarget, pick_file};

fn pick(files: &[&str], target: EpisodeTarget) -> Option<usize> {
    pick_file(files.iter().copied().enumerate(), &target)
}

#[test]
fn a_batch_yields_the_episode_file_past_creditless_samples_and_extras() {
    let files = [
        "[Grp] Frieren [1080p]/[Grp] Frieren - NCOP [1080p].mkv",
        "[Grp] Frieren [1080p]/[Grp] Frieren - NCED [1080p].mkv",
        "[Grp] Frieren [1080p]/Extras/[Grp] Frieren - 05 Making Of.mkv",
        "[Grp] Frieren [1080p]/[Grp] Frieren - 05 [1080p] sample.mkv",
        "[Grp] Frieren [1080p]/[Grp] Frieren - 04 [1080p].mkv",
        "[Grp] Frieren [1080p]/[Grp] Frieren - 05 [1080p].mkv",
    ];

    assert_eq!(pick(&files, EpisodeTarget::new(1, 5)), Some(5));
}

#[test]
fn a_batch_without_the_episode_yields_nothing() {
    let files = ["[Grp] Frieren - 01.mkv", "[Grp] Frieren - 02.mkv"];

    assert_eq!(pick(&files, EpisodeTarget::new(1, 5)), None);
}

#[test]
fn absolute_numbering_finds_the_file() {
    let files = ["[Grp] Shingeki - 59.mkv", "[Grp] Shingeki - 60.mkv"];
    let target = EpisodeTarget::new(4, 1).with_absolute_episode(60);

    assert_eq!(pick(&files, target), Some(1));
}

#[test]
fn seasonal_numbering_rejects_the_wrong_season() {
    let files = ["Show S01E05.mkv", "Show S02E05.mkv"];

    assert_eq!(pick(&files, EpisodeTarget::new(2, 5)), Some(1));
}

#[test]
fn subtitle_and_other_non_video_files_are_skipped() {
    let files = ["[Grp] Frieren - 05.ass", "[Grp] Frieren - 05.nfo"];

    assert_eq!(pick(&files, EpisodeTarget::new(1, 5)), None);
}

#[test]
fn the_highest_version_wins_among_copies_of_the_episode() {
    let files = [
        "[Grp] Frieren - 05.mkv",
        "[Grp] Frieren - 05v2.mkv",
        "[Grp] Frieren - 06.mkv",
    ];

    assert_eq!(pick(&files, EpisodeTarget::new(1, 5)), Some(1));
}

#[test]
fn a_lone_unnumbered_video_is_the_episode() {
    let files = ["frieren.mkv", "frieren.nfo"];

    assert_eq!(pick(&files, EpisodeTarget::new(1, 5)), Some(0));
}

#[test]
fn several_unnumbered_videos_are_ambiguous() {
    let files = ["a.mkv", "b.mkv"];

    assert_eq!(pick(&files, EpisodeTarget::new(1, 5)), None);
}

#[test]
fn file_indices_come_from_the_caller() {
    let files = [(3, "x - 04.mkv"), (7, "x - 05.mkv")];

    assert_eq!(pick_file(files, &EpisodeTarget::new(1, 5)), Some(7));
}
