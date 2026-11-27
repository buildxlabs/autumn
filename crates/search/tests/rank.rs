//! Auto-select hard filters, scoring order and tie-breaks (#181 Q8–Q9).

#![expect(
    clippy::indexing_slicing,
    reason = "tests compare the head of a ranking; a short ranking fails the test"
)]

use search::{EpisodeTarget, Indexer, Torrent, rank};
use settings::{AutoSelectProfile, Codec, Preference, Source};

const EPISODE_5: EpisodeTarget = EpisodeTarget::new(1, 5);

/// A Nyaa result with 10 seeders and 1 GB, the infohash padded from `hash`.
fn torrent(hash: char, name: &str) -> Torrent {
    let mut torrent = Torrent::new(Indexer::Nyaa, hash.to_string().repeat(40), name);
    torrent.seeders = Some(10);
    torrent.size = Some(1_000_000_000);
    torrent
}

fn names(ranked: &[Torrent]) -> Vec<&str> {
    ranked.iter().map(|torrent| torrent.name.as_str()).collect()
}

fn ranked(profile: &AutoSelectProfile, torrents: Vec<Torrent>) -> Vec<String> {
    names(&rank(torrents, &EPISODE_5, profile))
        .into_iter()
        .map(str::to_owned)
        .collect()
}

// ---- hard filters ----

#[test]
fn releases_of_another_episode_or_kind_are_dropped() {
    let kept = ranked(
        &AutoSelectProfile::default(),
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p]"),
            torrent('b', "[Grp] Frieren - 06 [1080p]"),
            torrent('c', "[Grp] Frieren - NCOP [1080p]"),
            torrent('d', "[Grp] Frieren S02E05 [1080p]"),
        ],
    );

    assert_eq!(kept, ["[Grp] Frieren - 05 [1080p]"]);
}

#[test]
fn excluded_terms_drop_releases_case_insensitively() {
    let profile = AutoSelectProfile {
        excluded_terms: vec!["cam".to_owned()],
        ..AutoSelectProfile::default()
    };

    let kept = ranked(
        &profile,
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p][CAM]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
        ],
    );

    assert_eq!(kept, ["[Grp] Frieren - 05 [1080p]"]);
}

#[test]
fn min_seeders_drops_fewer_and_unknown_seeders() {
    let profile = AutoSelectProfile {
        min_seeders: 5,
        ..AutoSelectProfile::default()
    };
    let mut few = torrent('a', "[Few] Frieren - 05");
    few.seeders = Some(4);
    let mut unknown = torrent('b', "[Unknown] Frieren - 05");
    unknown.seeders = None;
    let mut enough = torrent('c', "[Enough] Frieren - 05");
    enough.seeders = Some(5);

    assert_eq!(
        ranked(&profile, vec![few, unknown, enough]),
        ["[Enough] Frieren - 05"]
    );
}

#[test]
fn a_zero_min_seeders_keeps_unknown_seeders() {
    let mut unknown = torrent('a', "[Unknown] Frieren - 05");
    unknown.seeders = None;

    assert_eq!(
        ranked(&AutoSelectProfile::default(), vec![unknown]),
        ["[Unknown] Frieren - 05"]
    );
}

#[test]
fn size_bounds_drop_out_of_range_and_unknown_sizes() {
    let profile = AutoSelectProfile {
        min_size: Some(500),
        max_size: Some(1_000),
        ..AutoSelectProfile::default()
    };
    let sized = |hash, name, size| {
        let mut torrent = torrent(hash, name);
        torrent.size = size;
        torrent
    };

    let kept = ranked(
        &profile,
        vec![
            sized('a', "[Small] Frieren - 05", Some(499)),
            sized('b', "[Big] Frieren - 05", Some(1_001)),
            sized('c', "[Unknown] Frieren - 05", None),
            sized('d', "[Low] Frieren - 05", Some(500)),
            sized('e', "[High] Frieren - 05", Some(1_000)),
        ],
    );

    assert_eq!(kept, ["[Low] Frieren - 05", "[High] Frieren - 05"]);
}

#[test]
fn only_and_never_filter_dual_audio() {
    let releases = || {
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p][Dual Audio]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
        ]
    };
    let with = |multiple_audio| AutoSelectProfile {
        multiple_audio,
        ..AutoSelectProfile::default()
    };

    assert_eq!(
        ranked(&with(Preference::Only), releases()),
        ["[Grp] Frieren - 05 [1080p][Dual Audio]"]
    );
    assert_eq!(
        ranked(&with(Preference::Never), releases()),
        ["[Grp] Frieren - 05 [1080p]"]
    );
}

#[test]
fn only_and_never_filter_batches() {
    let releases = || {
        vec![
            torrent('a', "[Grp] Frieren (01-28) [1080p]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
        ]
    };
    let with = |batch| AutoSelectProfile {
        batch,
        ..AutoSelectProfile::default()
    };

    assert_eq!(
        ranked(&with(Preference::Only), releases()),
        ["[Grp] Frieren (01-28) [1080p]"]
    );
    assert_eq!(
        ranked(&with(Preference::Never), releases()),
        ["[Grp] Frieren - 05 [1080p]"]
    );
}

#[test]
fn an_indexer_batch_flag_counts_as_a_batch() {
    let mut flagged = torrent('a', "[Grp] Frieren Complete [1080p]");
    flagged.is_batch = true;
    let profile = AutoSelectProfile {
        batch: Preference::Never,
        ..AutoSelectProfile::default()
    };

    assert_eq!(ranked(&profile, vec![flagged]), Vec::<String>::new());
}

#[test]
fn only_and_never_filter_multiple_subs() {
    let releases = || {
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p][Multi-Subs]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
        ]
    };
    let with = |multiple_subs| AutoSelectProfile {
        multiple_subs,
        ..AutoSelectProfile::default()
    };

    assert_eq!(
        ranked(&with(Preference::Only), releases()),
        ["[Grp] Frieren - 05 [1080p][Multi-Subs]"]
    );
    assert_eq!(
        ranked(&with(Preference::Never), releases()),
        ["[Grp] Frieren - 05 [1080p]"]
    );
}

#[test]
fn require_codec_drops_other_and_unknown_codecs() {
    let profile = AutoSelectProfile {
        codecs: vec![Codec::H265],
        require_codec: true,
        ..AutoSelectProfile::default()
    };

    let kept = ranked(
        &profile,
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p x264]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
            torrent('c', "[Grp] Frieren - 05 [1080p HEVC]"),
        ],
    );

    assert_eq!(kept, ["[Grp] Frieren - 05 [1080p HEVC]"]);
}

#[test]
fn require_source_drops_other_and_unknown_sources() {
    let profile = AutoSelectProfile {
        sources: vec![Source::BluRay],
        require_source: true,
        ..AutoSelectProfile::default()
    };

    let kept = ranked(
        &profile,
        vec![
            torrent('a', "[Grp] Frieren - 05 [WEB-DL 1080p]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
            torrent('c', "[Grp] Frieren - 05 [BD 1080p]"),
        ],
    );

    assert_eq!(kept, ["[Grp] Frieren - 05 [BD 1080p]"]);
}

#[test]
fn require_language_drops_releases_naming_none_of_the_languages() {
    let profile = AutoSelectProfile {
        preferred_languages: vec!["VOSTFR".to_owned()],
        require_language: true,
        ..AutoSelectProfile::default()
    };

    let kept = ranked(
        &profile,
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p]"),
            torrent('b', "[Grp] Frieren - 05 VOSTFR [1080p]"),
        ],
    );

    assert_eq!(kept, ["[Grp] Frieren - 05 VOSTFR [1080p]"]);
}

// ---- scoring order: each criterion outranks every later one ----

#[test]
fn resolution_follows_the_profile_order_with_unknown_last() {
    let profile = AutoSelectProfile {
        resolutions: vec![720, 1080],
        ..AutoSelectProfile::default()
    };

    let order = ranked(
        &profile,
        vec![
            torrent('a', "[Grp] Frieren - 05"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
            torrent('c', "[Grp] Frieren - 05 [2160p]"),
            torrent('d', "[Grp] Frieren - 05 [720p]"),
        ],
    );

    assert_eq!(
        order[..2],
        ["[Grp] Frieren - 05 [720p]", "[Grp] Frieren - 05 [1080p]"]
    );
}

#[test]
fn resolution_outranks_release_group() {
    let profile = AutoSelectProfile {
        release_groups: vec!["Fav".to_owned()],
        ..AutoSelectProfile::default()
    };

    let order = ranked(
        &profile,
        vec![
            torrent('a', "[Fav] Frieren - 05 [720p]"),
            torrent('b', "[Other] Frieren - 05 [1080p]"),
        ],
    );

    assert_eq!(
        order,
        ["[Other] Frieren - 05 [1080p]", "[Fav] Frieren - 05 [720p]"]
    );
}

#[test]
fn release_groups_follow_the_profile_order_case_insensitively() {
    let profile = AutoSelectProfile {
        release_groups: vec!["second".to_owned(), "FIRST".to_owned()],
        ..AutoSelectProfile::default()
    };

    let order = ranked(
        &profile,
        vec![
            torrent('a', "[Other] Frieren - 05 [1080p]"),
            torrent('b', "[First] Frieren - 05 [1080p]"),
            torrent('c', "[Second] Frieren - 05 [1080p]"),
        ],
    );

    assert_eq!(
        order,
        [
            "[Second] Frieren - 05 [1080p]",
            "[First] Frieren - 05 [1080p]",
            "[Other] Frieren - 05 [1080p]",
        ]
    );
}

#[test]
fn release_group_outranks_codec() {
    let profile = AutoSelectProfile {
        release_groups: vec!["Fav".to_owned()],
        codecs: vec![Codec::H265],
        ..AutoSelectProfile::default()
    };

    let order = ranked(
        &profile,
        vec![
            torrent('a', "[Other] Frieren - 05 [1080p HEVC]"),
            torrent('b', "[Fav] Frieren - 05 [1080p x264]"),
        ],
    );

    assert_eq!(order[0], "[Fav] Frieren - 05 [1080p x264]");
}

#[test]
fn codec_outranks_seeders() {
    let profile = AutoSelectProfile {
        codecs: vec![Codec::H265],
        ..AutoSelectProfile::default()
    };
    let mut popular = torrent('a', "[Grp] Frieren - 05 [1080p x264]");
    popular.seeders = Some(500);

    let order = ranked(
        &profile,
        vec![popular, torrent('b', "[Grp] Frieren - 05 [1080p HEVC]")],
    );

    assert_eq!(order[0], "[Grp] Frieren - 05 [1080p HEVC]");
}

#[test]
fn seeders_outrank_size_and_unknown_seeders_rank_last() {
    let mut small = torrent('a', "[Small] Frieren - 05 [1080p]");
    small.size = Some(100);
    let mut popular = torrent('b', "[Popular] Frieren - 05 [1080p]");
    popular.seeders = Some(50);
    let mut unknown = torrent('c', "[Unknown] Frieren - 05 [1080p]");
    unknown.seeders = None;
    unknown.size = Some(1);

    let order = ranked(&AutoSelectProfile::default(), vec![unknown, small, popular]);

    assert_eq!(
        order,
        [
            "[Popular] Frieren - 05 [1080p]",
            "[Small] Frieren - 05 [1080p]",
            "[Unknown] Frieren - 05 [1080p]",
        ]
    );
}

#[test]
fn size_outranks_dual_audio_and_unknown_size_ranks_last() {
    let profile = AutoSelectProfile {
        multiple_audio: Preference::Prefer,
        ..AutoSelectProfile::default()
    };
    let mut small = torrent('a', "[Grp] Frieren - 05 [1080p]");
    small.size = Some(100);
    let dual = torrent('b', "[Grp] Frieren - 05 [1080p][Dual Audio]");
    let mut unknown = torrent('c', "[Unknown] Frieren - 05 [1080p][Dual Audio]");
    unknown.size = None;

    let order = ranked(&profile, vec![unknown, dual, small]);

    assert_eq!(
        order,
        [
            "[Grp] Frieren - 05 [1080p]",
            "[Grp] Frieren - 05 [1080p][Dual Audio]",
            "[Unknown] Frieren - 05 [1080p][Dual Audio]",
        ]
    );
}

#[test]
fn dual_audio_prefer_and_avoid_outrank_batch() {
    let releases = || {
        vec![
            torrent('a', "[Grp] Frieren (01-28) [1080p]"),
            torrent('b', "[Grp] Frieren - 05 [1080p][Dual Audio]"),
        ]
    };
    let with = |multiple_audio| AutoSelectProfile {
        multiple_audio,
        batch: Preference::Prefer,
        ..AutoSelectProfile::default()
    };

    assert_eq!(
        ranked(&with(Preference::Prefer), releases())[0],
        "[Grp] Frieren - 05 [1080p][Dual Audio]"
    );
    assert_eq!(
        ranked(&with(Preference::Avoid), releases())[0],
        "[Grp] Frieren (01-28) [1080p]"
    );
}

#[test]
fn batch_prefer_and_avoid_order_batches() {
    let releases = || {
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p]"),
            torrent('b', "[Grp] Frieren (01-28) [1080p]"),
        ]
    };
    let with = |batch| AutoSelectProfile {
        batch,
        ..AutoSelectProfile::default()
    };

    assert_eq!(
        ranked(&with(Preference::Prefer), releases())[0],
        "[Grp] Frieren (01-28) [1080p]"
    );
    assert_eq!(
        ranked(&with(Preference::Avoid), releases())[0],
        "[Grp] Frieren - 05 [1080p]"
    );
}

// ---- tie-breaks ----

#[test]
fn ties_prefer_the_highest_version() {
    let order = ranked(
        &AutoSelectProfile::default(),
        vec![
            torrent('a', "[Grp] Frieren - 05 [1080p]"),
            torrent('b', "[Grp] Frieren - 05v3 [1080p]"),
            torrent('c', "[Grp] Frieren - 05v2 [1080p]"),
        ],
    );

    assert_eq!(
        order,
        [
            "[Grp] Frieren - 05v3 [1080p]",
            "[Grp] Frieren - 05v2 [1080p]",
            "[Grp] Frieren - 05 [1080p]",
        ]
    );
}

#[test]
fn version_ties_prefer_non_remakes() {
    let mut remake = torrent('a', "[Remake] Frieren - 05 [1080p]");
    remake.remake = true;

    let order = ranked(
        &AutoSelectProfile::default(),
        vec![remake, torrent('b', "[Original] Frieren - 05 [1080p]")],
    );

    assert_eq!(order[0], "[Original] Frieren - 05 [1080p]");
}

#[test]
fn full_ties_order_by_infohash() {
    let order = rank(
        vec![
            torrent('c', "[Grp] Frieren - 05 [1080p]"),
            torrent('a', "[Grp] Frieren - 05 [1080p]"),
            torrent('b', "[Grp] Frieren - 05 [1080p]"),
        ],
        &EPISODE_5,
        &AutoSelectProfile::default(),
    );

    let hashes: Vec<char> = order
        .iter()
        .filter_map(|torrent| torrent.infohash.chars().next())
        .collect();
    assert_eq!(hashes, ['a', 'b', 'c']);
}

#[test]
fn version_outranks_remake() {
    let mut remake = torrent('a', "[Grp] Frieren - 05v2 [1080p]");
    remake.remake = true;

    let order = ranked(
        &AutoSelectProfile::default(),
        vec![torrent('b', "[Grp] Frieren - 05 [1080p]"), remake],
    );

    assert_eq!(order[0], "[Grp] Frieren - 05v2 [1080p]");
}

#[test]
fn optional_language_and_source_rank_only_after_batch() {
    let profile = AutoSelectProfile {
        batch: Preference::Prefer,
        preferred_languages: vec!["VOSTFR".to_owned()],
        sources: vec![Source::BluRay],
        ..AutoSelectProfile::default()
    };

    let order = ranked(
        &profile,
        vec![
            torrent('a', "[Grp] Frieren - 05 VOSTFR [1080p]"),
            torrent('b', "[Grp] Frieren - 05 [BD 1080p]"),
            torrent('c', "[Grp] Frieren (01-28) [1080p]"),
            torrent('d', "[Grp] Frieren - 05 [1080p]"),
        ],
    );

    assert_eq!(
        order,
        [
            "[Grp] Frieren (01-28) [1080p]",
            "[Grp] Frieren - 05 VOSTFR [1080p]",
            "[Grp] Frieren - 05 [BD 1080p]",
            "[Grp] Frieren - 05 [1080p]",
        ]
    );
}
