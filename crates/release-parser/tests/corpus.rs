//! Parses every name in `fixtures/corpus.txt` and reports each mismatch.
//!
//! The corpus is blocks of `key: value` lines separated by blank lines. Each
//! block starts with `name:`. A field the block leaves out is expected to be
//! absent, and `kind` defaults to `episode`. Lines starting with `#` are
//! comments. See `fixtures/SOURCES.md` for where the names come from.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use release_parser::{Release, parse};

const CORPUS: &str = include_str!("fixtures/corpus.txt");

const FIELDS: [&str; 11] = [
    "title",
    "group",
    "season",
    "episode",
    "version",
    "episode_title",
    "kind",
    "resolution",
    "year",
    "checksum",
    "extension",
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
            FIELDS.contains(&key),
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
    BTreeMap::from([
        ("title", release.title.clone()),
        ("group", release.group.clone()),
        ("season", number(release.season)),
        ("episode", number(release.episode)),
        ("version", number(release.version)),
        ("episode_title", release.episode_title.clone()),
        ("kind", Some(format!("{:?}", release.kind).to_lowercase())),
        ("resolution", number(release.resolution)),
        ("year", number(release.year)),
        ("checksum", release.checksum.map(|crc| format!("{crc:08X}"))),
        ("extension", release.extension.clone()),
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
        for field in FIELDS {
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
fn corpus_holds_at_least_150_distinct_names() {
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
    assert!(cases.len() >= 150, "only {} names", cases.len());
}
