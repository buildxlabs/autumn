//! Parses every name in `fixtures/corpus.txt` and reports each mismatch.
//!
//! The corpus is blocks of `key: value` lines separated by blank lines. Each
//! block starts with `name:`. A field the block leaves out is expected to be
//! absent, `kind` defaults to `episode` and `batch` to false. Lines starting
//! with `#` are comments. See `fixtures/SOURCES.md` for where the names come
//! from.
//!
//! The technical tag fields are checked only in blocks that give at least one
//! of them, or `tags: none` for a name without tags. Lists such as `audio` are
//! comma-separated, in the order the name gives them.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use release_parser::{Release, parse};

const CORPUS: &str = include_str!("fixtures/corpus.txt");

/// Fields every block is checked on.
const FIELDS: [&str; 15] = [
    "title",
    "group",
    "season",
    "last_season",
    "episode",
    "last_episode",
    "version",
    "volume",
    "batch",
    "episode_title",
    "kind",
    "resolution",
    "year",
    "checksum",
    "extension",
];

/// Fields checked only in blocks that opt in with a tag field or `tags: none`.
const TAG_FIELDS: [&str; 6] = [
    "video_codec",
    "bit_depth",
    "audio",
    "source",
    "streaming",
    "subtitles",
];

/// One corpus entry: the release name and its expected fields.
struct Case {
    line: usize,
    name: String,
    expected: BTreeMap<String, String>,
}

fn cases() -> Vec<Case> {
    let mut cases: Vec<Case> = Vec::new();
    for (index, line) in CORPUS.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = line
            .split_once(": ")
            .unwrap_or_else(|| (line.trim_end_matches(':'), ""));
        if key == "name" {
            cases.push(Case {
                line: line_number,
                name: value.to_owned(),
                expected: BTreeMap::new(),
            });
            continue;
        }
        assert!(
            FIELDS.contains(&key) || TAG_FIELDS.contains(&key) || line == "tags: none",
            "corpus line {line_number}: unknown field {key:?}"
        );
        assert!(
            !cases.is_empty(),
            "corpus line {line_number}: {key} before any name"
        );
        if let Some(case) = cases.last_mut() {
            let previous = case.expected.insert(key.to_owned(), value.to_owned());
            assert!(
                previous.is_none(),
                "corpus line {line_number}: {key} given twice"
            );
        }
    }
    cases
}

/// Each field rendered the way the corpus writes it.
fn actual(release: &Release) -> BTreeMap<&'static str, Option<String>> {
    let number = |n: Option<u32>| n.map(|n| n.to_string());
    let name = |debug: String| debug.to_lowercase();
    let list = |names: Vec<String>| (!names.is_empty()).then(|| names.join(", "));
    BTreeMap::from([
        ("title", release.title.clone()),
        ("group", release.group.clone()),
        ("season", number(release.season)),
        ("last_season", number(release.last_season)),
        ("episode", number(release.episode)),
        ("last_episode", number(release.last_episode)),
        ("version", number(release.version)),
        ("volume", number(release.volume)),
        ("batch", release.batch.then(|| "true".to_owned())),
        ("episode_title", release.episode_title.clone()),
        ("kind", Some(format!("{:?}", release.kind).to_lowercase())),
        ("resolution", number(release.resolution)),
        ("year", number(release.year)),
        ("checksum", release.checksum.map(|crc| format!("{crc:08X}"))),
        ("extension", release.extension.clone()),
        (
            "video_codec",
            release.video_codec.map(|c| name(format!("{c:?}"))),
        ),
        ("bit_depth", number(release.bit_depth)),
        (
            "audio",
            list(
                release
                    .audio
                    .iter()
                    .map(|a| name(format!("{a:?}")))
                    .collect(),
            ),
        ),
        ("source", release.source.map(|s| name(format!("{s:?}")))),
        (
            "streaming",
            release.streaming.map(|s| name(format!("{s:?}"))),
        ),
        (
            "subtitles",
            list(
                release
                    .subtitles
                    .iter()
                    .map(|s| name(format!("{s:?}")))
                    .collect(),
            ),
        ),
    ])
}

#[test]
fn every_corpus_name_parses_to_its_expected_fields() {
    let cases = cases();
    let mut report = String::new();
    let mut failed = 0;
    for case in &cases {
        let actual = actual(&parse(&case.name));
        let mut mismatches = String::new();
        let tags_checked = case.expected.contains_key("tags")
            || TAG_FIELDS.iter().any(|f| case.expected.contains_key(*f));
        let checked = FIELDS
            .iter()
            .chain(TAG_FIELDS.iter().filter(|_| tags_checked));
        for &field in checked {
            let expected = match case.expected.get(field) {
                Some(value) => Some(value.as_str()),
                None if field == "kind" => Some("episode"),
                None => None,
            };
            let got = actual.get(field).cloned().flatten();
            if expected != got.as_deref() {
                writeln!(
                    mismatches,
                    "    {field}: expected {expected:?}, got {got:?}"
                )
                .unwrap();
            }
        }
        if !mismatches.is_empty() {
            failed += 1;
            writeln!(report, "line {}: {}\n{mismatches}", case.line, case.name).unwrap();
        }
    }
    assert!(
        failed == 0,
        "{failed} of {} corpus names parse wrong:\n\n{report}",
        cases.len()
    );
}

#[test]
fn corpus_holds_at_least_500_distinct_names_from_40_groups() {
    let cases = cases();
    let mut seen = BTreeMap::new();
    for case in &cases {
        let first = seen.insert(case.name.as_str(), case.line);
        assert!(
            first.is_none(),
            "line {}: {:?} repeats line {}",
            case.line,
            case.name,
            first.unwrap_or_default()
        );
    }
    assert!(cases.len() >= 500, "only {} names", cases.len());
    let groups: std::collections::BTreeSet<&str> = cases
        .iter()
        .filter_map(|case| case.expected.get("group").map(String::as_str))
        .collect();
    assert!(groups.len() >= 40, "only {} release groups", groups.len());
}
