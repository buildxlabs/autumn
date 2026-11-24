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

#[test]
fn ambiguous_tag_words_stay_in_the_title() {
    for (name, title) in [
        ("[Group] DD Hokuto no Ken - 01", "DD Hokuto no Ken"),
        ("[Group] Magnum Opus - 01", "Magnum Opus"),
        ("[Group] Web Title - 01", "Web Title"),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
        assert_eq!(release.episode, Some(1), "{name}");
    }
}

#[test]
fn dictionary_kind_word_inside_a_title_is_part_of_it() {
    let release = parse("[Group] Happy Ending - 01");

    assert_eq!(release.title.as_deref(), Some("Happy Ending"));
    assert_eq!(release.kind, EpisodeKind::Episode);
    assert_eq!(release.episode, Some(1));
}

#[test]
fn leading_gekijouban_marks_a_movie() {
    let release = parse("[Group] Gekijouban Kimetsu no Yaiba Mugen Ressha-hen [1080p]");

    assert_eq!(
        release.title.as_deref(),
        Some("Kimetsu no Yaiba Mugen Ressha-hen")
    );
    assert_eq!(release.kind, EpisodeKind::Movie);
}

#[test]
fn the_movie_marks_a_movie_and_leaves_the_title() {
    let release = parse("[Group] Kimetsu no Yaiba: The Movie - Mugen Train [1080p]");

    assert_eq!(release.title.as_deref(), Some("Kimetsu no Yaiba"));
    assert_eq!(release.kind, EpisodeKind::Movie);
    assert_eq!(release.episode_title.as_deref(), Some("Mugen Train"));
}

#[test]
fn kind_word_in_a_bracket_sets_the_kind() {
    for name in ["[Group] Title (OVA) - 01", "[Group] Title - 01 (OVA)"] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.kind, EpisodeKind::Special, "{name}");
        assert_eq!(release.episode, Some(1), "{name}");
    }
}

#[test]
fn kind_word_after_the_episode_number_sets_the_kind() {
    let release = parse("[Group] Title - 05 Preview [1080p]");

    assert_eq!(release.kind, EpisodeKind::Extra);
    assert_eq!(release.episode, Some(5));
    assert_eq!(release.episode_title, None);
}

#[test]
fn empty_first_bracket_is_no_group() {
    let release = parse("[] Title - 01");

    assert_eq!(release.group, None);
    assert_eq!(release.title.as_deref(), Some("Title"));
}

#[test]
fn four_digit_number_closing_a_folder_name_stays_in_the_title() {
    let release = parse("Mobile Suit Gundam 0079");

    assert_eq!(release.title.as_deref(), Some("Mobile Suit Gundam 0079"));
    assert_eq!(release.episode, None);
}

#[test]
fn movie_names_give_the_kind_year_and_subtitle() {
    for (name, title, number, episode_title, year) in [
        (
            "[Group] Title Movie (2019) (BD 1080p)",
            "Title",
            None,
            None,
            Some(2019),
        ),
        (
            "[Group] Title the Movie Part III Rebellion (2013) 1080p",
            "Title",
            None,
            Some("Part III Rebellion"),
            Some(2013),
        ),
        (
            "[Group] Title Memorial Movie 2023 (BD 1080p)",
            "Title Memorial",
            None,
            None,
            Some(2023),
        ),
        (
            "[Group] Doraemon Movie 20 - Nobita's Space (1999) [1080p]",
            "Doraemon",
            Some(20),
            Some("Nobita's Space"),
            Some(1999),
        ),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
        assert_eq!(release.kind, EpisodeKind::Movie, "{name}");
        assert_eq!(release.episode, number, "{name}");
        assert_eq!(release.episode_title.as_deref(), episode_title, "{name}");
        assert_eq!(release.year, year, "{name}");
    }
}

#[test]
fn standalone_version_word_after_the_title_is_the_version() {
    for (name, title, version) in [
        (
            "[Sokudo] The Promised Neverland S01 v3 [1080p BD AV1]",
            "The Promised Neverland",
            3,
        ),
        (
            "[Group] Title (2006) - Season 1 S01 v2 (BD 576p)",
            "Title",
            2,
        ),
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some(title), "{name}");
        assert_eq!(release.season, Some(1), "{name}");
        assert_eq!(release.version, Some(version), "{name}");
    }
}

#[test]
fn version_words_anywhere_before_the_tags() {
    for name in [
        "[Group] Title - 01 v2 [1080p]",
        "[Group] Title v2 - 01 [1080p]",
    ] {
        let release = parse(name);

        assert_eq!(release.title.as_deref(), Some("Title"), "{name}");
        assert_eq!(release.episode, Some(1), "{name}");
        assert_eq!(release.version, Some(2), "{name}");
        assert_eq!(release.episode_title, None, "{name}");
    }
}

#[test]
fn a_tag_bracket_after_the_title_closes_it() {
    let release =
        parse("[Moozzi2] Kingdom S1 [ キングダム S1 ] (BD 1920x1080 x265-10Bit Flac) - TV + SP");

    assert_eq!(release.title.as_deref(), Some("Kingdom"));
    assert_eq!(release.season, Some(1));
    assert_eq!(release.kind, EpisodeKind::Episode);

    let release = parse("Attack on Titan [1080p] - E01.mkv");
    assert_eq!(release.title.as_deref(), Some("Attack on Titan"));
    assert_eq!(release.episode, Some(1));
}
