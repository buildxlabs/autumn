//! Episode matching rules from #181 Q5–Q7 and ADR 0007.

use release_parser::parse;
use search::{EpisodeTarget, Indexer, Torrent, matches, matches_torrent};

fn target(season: u32, episode: u32) -> EpisodeTarget {
    EpisodeTarget::new(season, episode)
}

#[test]
fn seasonal_and_plain_episode_numbers_match() {
    let t = target(1, 5);
    for name in [
        "[Judas] Baccano! - S01E05.mkv",
        "[SubsPlease] Frieren - 05 (1080p).mkv",
    ] {
        assert!(matches(&parse(name), &t), "{name}");
    }
}

#[test]
fn versioned_names_match_the_same_episode() {
    assert!(matches(
        &parse("[Judas] Kimetsu no Yaiba - S05E01v2.mkv"),
        &target(5, 1)
    ));
    assert!(matches(
        &parse("[SubsPlease] 86 - Eighty Six - 20v2 (1080p) [30072859].mkv"),
        &target(1, 20)
    ));
}

#[test]
fn a_release_without_an_episode_number_does_not_match() {
    assert!(!matches(
        &parse("[Commie] Kizumonogatari I - Tekketsu-hen [1080p].mkv"),
        &target(1, 1)
    ));
}

#[test]
fn a_different_episode_number_does_not_match() {
    assert!(!matches(
        &parse("[SubsPlease] Frieren - 04 (1080p).mkv"),
        &target(1, 5)
    ));
}

#[test]
fn specials_movies_and_creditless_videos_never_match_an_episode() {
    let t = target(1, 1);
    for name in [
        "[SubsPlease] Mushoku Tensei S2 - 00 (1080p) [9C362DC3].mkv",
        "[Group] Frieren SP01 (1080p).mkv",
        "[Group] Frieren OVA - 01 [1080p].mkv",
        "[Group] Frieren NCOP - 01 [1080p].mkv",
        "[Group] Frieren NCED - 01 [1080p].mkv",
        "[Group] Kimetsu no Yaiba: The Movie - Mugen Train [1080p].mkv",
        "[Group] Frieren - 01 Preview [1080p].mkv",
    ] {
        assert!(!matches(&parse(name), &t), "{name}");
    }
}

#[test]
fn a_known_wrong_season_is_rejected() {
    let release = parse("[Erai-raws] Danmachi S5 - 01 (1080p).mkv");
    assert_eq!(release.season, Some(5));
    assert!(!matches(&release, &target(1, 1)));
    assert!(matches(&release, &target(5, 1)));
}

#[test]
fn a_season_range_covers_the_media_season() {
    let batch = parse("[Group] Frieren S01-S03 (EP01-12) 1080p WEB-DL");
    assert!(matches(&batch, &target(1, 5)));
    assert!(!matches(&batch, &target(4, 5)));
    let season_only = parse("[Group] Frieren S01-S03 [1080p]");
    assert!(matches(&season_only, &target(1, 5)));
    assert!(!matches(&season_only, &target(4, 5)));
}

#[test]
fn a_missing_season_marker_is_accepted_when_the_number_matches() {
    assert!(matches(
        &parse("[SubsPlease] Frieren - 01 (1080p).mkv"),
        &target(2, 1)
    ));
}

#[test]
fn absolute_numbering_matches_when_it_differs_from_the_seasonal_number() {
    let t = EpisodeTarget::new(1, 2).with_absolute_episode(35);
    assert!(matches(
        &parse("[SubsPlease] One Piece - 35 (1080p).mkv"),
        &t
    ));
    // Both numberings are accepted, but a number that is neither does not match.
    assert!(!matches(
        &parse("[SubsPlease] One Piece - 03 (1080p).mkv"),
        &t
    ));
}

#[test]
fn an_absolute_number_still_rejects_a_known_wrong_season() {
    let t = EpisodeTarget::new(1, 2).with_absolute_episode(35);
    assert!(!matches(
        &parse("[Group] One Piece S03 - 35 (1080p).mkv"),
        &t
    ));
}

#[test]
fn an_episode_range_covers_the_requested_episode() {
    let batch = parse("[Group] Frieren - 01-12 (1080p)");
    assert!(matches(&batch, &target(1, 5)));
    assert!(!matches(&batch, &target(1, 13)));
}

#[test]
fn a_flagged_batch_without_a_range_matches_but_a_contradicting_range_does_not() {
    let complete = parse("[Group] Frieren Complete [1080p]");
    assert!(matches(&complete, &target(1, 5)));
    let ranged = parse("[Group] Frieren 01-03 Batch [1080p]");
    assert!(!matches(&ranged, &target(1, 5)));
}

#[test]
fn an_indexer_batch_flag_confirms_a_release_without_a_number() {
    let mut torrent = Torrent::new(
        Indexer::AnimeTosho,
        "a".repeat(40),
        "[Group] Frieren [1080p]",
    );
    assert!(!matches_torrent(&torrent, &target(1, 5)));
    torrent.is_batch = true;
    assert!(matches_torrent(&torrent, &target(1, 5)));
}

#[test]
fn a_batch_of_a_special_kind_never_matches_an_episode() {
    let batch = parse("[Group] Frieren (OVA) 01-03 Batch [1080p]");
    assert!(!matches(&batch, &target(1, 2)));
}
