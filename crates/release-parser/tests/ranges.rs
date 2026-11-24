//! Episode ranges, season ranges, volumes and the batch flag.

use release_parser::parse;

#[test]
fn every_episode_range_form_gives_first_and_last() {
    for (name, title, first, last) in [
        ("[Group] Title - 01-12 (1080p)", "Title", 1, 12),
        ("[Group] Title - 01~12 [1080p]", "Title", 1, 12),
        ("[Group] Title - E01-E12 [1080p]", "Title", 1, 12),
        ("[Group] Title (01-28) (1080p)", "Title", 1, 28),
        ("[Group] Title [01~12][1080p]", "Title", 1, 12),
        ("Title S01E14-E26 1080p WEB-DL", "Title", 14, 26),
        ("[Group] Title 2026 01-07 (WEBRip 1280x720)", "Title", 1, 7),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
        assert_eq!(release.episode, Some(first), "{name}");
        assert_eq!(release.last_episode, Some(last), "{name}");
        assert!(release.batch, "{name}: a range is a batch");
    }
}

#[test]
fn season_range_gives_first_and_last_season() {
    for name in [
        "[Group] Title S01-S03 [1080p]",
        "Title.S01-S03.1080p.BluRay.x265-GRP",
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.season, Some(1), "{name}");
        assert_eq!(release.last_season, Some(3), "{name}");
        assert!(release.batch, "{name}");
    }
}

#[test]
fn batch_and_complete_markers_set_the_batch_flag() {
    for name in [
        "[Group] Title Batch (1080p)",
        "[Group] Title (Season 1) [BD 1080p] (Batch)",
        "[Group] Title [Batch]",
        "[Group] Title Complete [1080p]",
        "[Group] Title (Complete) [1080p]",
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert!(release.batch, "{name}");
        assert_eq!(release.episode, None, "{name}");
    }
}

#[test]
fn volume_marker_gives_the_volume_and_a_batch() {
    for name in [
        "[Group] Title Vol.1 (BDRip 1920x1080)",
        "[Group] Title Vol 1 (BDRip 1920x1080)",
        "[Group] Title Vol. 01 (BDRip 1920x1080)",
        "[Group] Title S3 Vol 1 (BDRip 1920x1080)",
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.volume, Some(1), "{name}");
        assert!(release.batch, "{name}");
    }
}

#[test]
fn single_episode_is_not_a_batch() {
    let release = parse("[SubsPlease] Sousou no Frieren - 01 (1080p) [ABCD1234].mkv");

    assert!(!release.batch);
    assert_eq!(release.last_episode, None);
    assert_eq!(release.last_season, None);
    assert_eq!(release.volume, None);
}

#[test]
fn episode_in_its_own_bracket_with_a_prefix() {
    let release = parse("[Group] Title [Ep 01] [1080p]");

    assert_eq!(release.title.as_deref(), Some("Title"));
    assert_eq!(release.episode, Some(1));
}

#[test]
fn ranges_and_seasons_inside_wider_brackets() {
    for (name, season, last_season, first, last) in [
        (
            "[Group] Title [01-12 Batch][WebRip 1080p]",
            None,
            None,
            Some(1),
            Some(12),
        ),
        (
            "[Group] Title COMPLETE (001-155) [480p]",
            None,
            None,
            Some(1),
            Some(155),
        ),
        (
            "[Group] Title (S2 Complete) (1080p)",
            Some(2),
            None,
            None,
            None,
        ),
        (
            "[Group] Title (S01-S02) [BD 1080p]",
            Some(1),
            Some(2),
            None,
            None,
        ),
        (
            "[Group] Title 001-203 (S01-S09) (BD 480p)",
            Some(1),
            Some(9),
            Some(1),
            Some(203),
        ),
        (
            "[Group] Title S01-S06 (EP01-72) 1080p WEB-DL",
            Some(1),
            Some(6),
            Some(1),
            Some(72),
        ),
        ("[Group] Title 14-25 Batch", None, None, Some(14), Some(25)),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.season, season, "{name}");
        assert_eq!(release.last_season, last_season, "{name}");
        assert_eq!(release.episode, first, "{name}");
        assert_eq!(release.last_episode, last, "{name}");
        assert!(release.batch, "{name}");
    }
}

#[test]
fn volume_ranges_and_bracketed_volumes_give_the_first_volume() {
    for (name, first) in [
        ("[Group] Title Vol 1-2 (BDRip 1920x1080)", 1),
        ("[Group] Title Vol.2~3 [BDRip 1080p]", 2),
        ("Title 10-12 (BD 1080p) (Vol. 4)", 4),
        ("Title 01-06 (BD 1080p) (Vol. 1-2)", 1),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.volume, Some(first), "{name}");
        assert!(release.batch, "{name}");
    }
}

#[test]
fn a_number_range_inside_the_title_stays_in_it() {
    for (name, title) in [
        ("[Group] Ranma 1-2 (2024) - 01", "Ranma 1-2"),
        ("Touhou 7-8 Collection - 01", "Touhou 7-8 Collection"),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
        assert_eq!(release.episode, Some(1), "{name}");
        assert_eq!(release.last_episode, None, "{name}");
        assert!(!release.batch, "{name}");
    }
}

#[test]
fn spelled_out_and_full_season_episode_ranges() {
    for name in [
        "[Group] Title Season 1-3 [1080p]",
        "[Group] Title [Season 1-3] [1080p]",
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.season, Some(1), "{name}");
        assert_eq!(release.last_season, Some(3), "{name}");
        assert!(release.batch, "{name}");
    }

    let release = parse("Title S02E01-S02E12 1080p WEB-DL");
    assert_eq!(release.season, Some(2));
    assert_eq!(release.episode, Some(1));
    assert_eq!(release.last_episode, Some(12));
    assert_eq!(release.episode_title, None);
}

#[test]
fn batch_words_inside_a_title_stay_in_it() {
    for (name, title, episode) in [
        ("[Group] Complete Clear - 01", "Complete Clear", Some(1)),
        (
            "[Group] Title Complete Series - 01",
            "Title Complete Series",
            Some(1),
        ),
        (
            "[Group] Title The Complete Series",
            "Title The Complete Series",
            None,
        ),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
        assert_eq!(release.episode, episode, "{name}");
        assert!(!release.batch, "{name}");
    }
}
