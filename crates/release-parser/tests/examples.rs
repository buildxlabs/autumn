//! The release names the ticket names, one test each.

use release_parser::{EpisodeKind, parse};

#[test]
fn fansub_name_gives_group_title_episode_and_checksum() {
    let release = parse("[SubsPlease] Sousou no Frieren - 01 (1080p) [ABCD1234].mkv");

    assert_eq!(release.group.as_deref(), Some("SubsPlease"));
    assert_eq!(release.title.as_deref(), Some("Sousou no Frieren"));
    assert_eq!(release.episode, Some(1));
    assert_eq!(release.resolution, Some(1080));
    assert_eq!(release.checksum, Some(0xABCD_1234));
    assert_eq!(release.extension.as_deref(), Some("mkv"));
    assert_eq!(release.kind, EpisodeKind::Episode);
}

#[test]
fn season_episode_name_gives_season_and_episode() {
    let release = parse("Frieren S01E05 1080p.mkv");

    assert_eq!(release.title.as_deref(), Some("Frieren"));
    assert_eq!(release.season, Some(1));
    assert_eq!(release.episode, Some(5));
    assert_eq!(release.resolution, Some(1080));
    assert_eq!(release.group, None);
    assert_eq!(release.kind, EpisodeKind::Episode);
}

#[test]
fn numbered_creditless_opening_is_an_ncop_with_its_number() {
    let release = parse("[Group] Title - NCOP2 [BD 1080p].mkv");

    assert_eq!(release.title.as_deref(), Some("Title"));
    assert_eq!(release.kind, EpisodeKind::Ncop);
    assert_eq!(release.episode, Some(2));
    assert_eq!(release.resolution, Some(1080));
}

#[test]
fn bare_folder_name_is_all_title() {
    let release = parse("Sousou no Frieren");

    assert_eq!(release.title.as_deref(), Some("Sousou no Frieren"));
    assert_eq!(release.episode, None);
    assert_eq!(release.extension, None);
    assert_eq!(release.group, None);
    assert_eq!(release.kind, EpisodeKind::Episode);
}

#[test]
fn folder_name_ending_in_a_number_keeps_it_in_the_title() {
    let release = parse("Mob Psycho 100");

    assert_eq!(release.title.as_deref(), Some("Mob Psycho 100"));
    assert_eq!(release.episode, None);
}
