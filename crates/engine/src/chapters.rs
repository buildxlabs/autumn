//! Chapter skip markers: pattern matching and the mkv/player fallback (#131).
//!
//! A local Matroska file carries its chapters in the container. Other sources
//! (and files without an mkv chapter atom) use the player's `chapter-list`.

use std::{io::BufReader, path::Path, time::Duration};

use domain::{ChapterKind, PlaybackChapter};
use regex::Regex;

/// Case-insensitive patterns compiled once from the Playback settings.
#[derive(Debug, Clone)]
pub(crate) struct SkipPatterns {
    opening: Vec<Regex>,
    ending: Vec<Regex>,
}

impl SkipPatterns {
    /// Compiles the configured patterns. An invalid pattern is ignored: the
    /// Settings input accepts free text while the user is still typing.
    pub(crate) fn new(opening: &[String], ending: &[String]) -> Self {
        let compile = |patterns: &[String]| {
            patterns
                .iter()
                .filter_map(|pattern| Regex::new(&format!("(?i){pattern}")).ok())
                .collect()
        };
        Self {
            opening: compile(opening),
            ending: compile(ending),
        }
    }

    /// The kind a chapter title matches, ending taking precedence so an
    /// "Ending" chapter is never treated as an "Intro".
    pub(crate) fn classify(&self, title: Option<&str>) -> Option<ChapterKind> {
        let title = title?;
        if self.ending.iter().any(|pattern| pattern.is_match(title)) {
            return Some(ChapterKind::Ending);
        }
        if self.opening.iter().any(|pattern| pattern.is_match(title)) {
            return Some(ChapterKind::Opening);
        }
        None
    }

    fn attach(&self, mut chapters: Vec<PlaybackChapter>) -> Vec<PlaybackChapter> {
        for chapter in &mut chapters {
            chapter.kind = self.classify(chapter.title.as_deref());
        }
        chapters
    }
}

/// Converts one Matroska edition into skip markers.
///
/// Ordered editions use per-chapter times that do not line up with mpv's
/// timeline, so they are left to the player's `chapter-list`. Hidden chapters
/// are not skip markers either.
fn from_edition(edition: &mkv::Edition, patterns: &SkipPatterns) -> Vec<PlaybackChapter> {
    if edition.ordered {
        return Vec::new();
    }
    let chapters = edition
        .chapters
        .iter()
        .filter(|chapter| !chapter.hidden)
        .map(|chapter| {
            let name = chapter.display.first().map(|display| display.name.clone());
            let start = Duration::from_nanos(chapter.start);
            let end = (chapter.end > chapter.start).then(|| Duration::from_nanos(chapter.end));
            PlaybackChapter::new(name, start, end, None)
        })
        .collect();
    patterns.attach(chapters)
}

/// Reads the chapters of a local Matroska file, using the first visible edition.
///
/// # Errors
///
/// Returns the mkv error when the file is unreadable or not a Matroska file.
pub(crate) fn read_mkv(
    path: &Path,
    patterns: &SkipPatterns,
) -> Result<Vec<PlaybackChapter>, mkv::Error> {
    let file = std::fs::File::open(path)?;
    // The parse reads attachments too, so buffer the unbuffered file.
    let parsed = mkv::File::parse(BufReader::new(file))?;
    let edition = parsed
        .editions()
        .iter()
        .find(|edition| !edition.hidden)
        .or_else(|| parsed.editions().first());
    let Some(edition) = edition else {
        return Ok(Vec::new());
    };
    Ok(from_edition(edition, patterns))
}

/// Converts the player's `chapter-list` into the same plain model.
pub(crate) fn from_player(
    list: &[player::Chapter],
    patterns: &SkipPatterns,
) -> Vec<PlaybackChapter> {
    let chapters = list
        .iter()
        .filter(|chapter| chapter.start.is_finite() && chapter.start >= 0.0)
        .map(|chapter| {
            PlaybackChapter::new(
                chapter.title.clone(),
                Duration::from_secs_f64(chapter.start),
                None,
                None,
            )
        })
        .collect();
    patterns.attach(chapters)
}

/// Uses the container's chapters when it has any, otherwise the player's.
pub(crate) fn prefer_mkv(
    from_container: Vec<PlaybackChapter>,
    from_player: Vec<PlaybackChapter>,
) -> Vec<PlaybackChapter> {
    if from_container.is_empty() {
        from_player
    } else {
        from_container
    }
}

/// Fills in missing chapter ends from the next chapter's start.
pub(crate) fn with_ends(
    mut chapters: Vec<PlaybackChapter>,
    duration: Option<Duration>,
) -> Vec<PlaybackChapter> {
    for index in 0..chapters.len() {
        let next_start = chapters.get(index + 1).map(|next| next.start);
        if let Some(chapter) = chapters.get_mut(index)
            && chapter.end.is_none()
        {
            chapter.end = next_start.or(duration);
        }
    }
    chapters
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use domain::ChapterKind;

    use super::{SkipPatterns, from_edition, from_player, prefer_mkv, with_ends};

    #[derive(Clone, Copy)]
    enum EditionOrder {
        Ordered,
        Unordered,
    }

    fn edition(order: EditionOrder) -> mkv::Edition {
        let chapter = |name: &str, start: u64, end: u64, hidden: bool| mkv::Chapter {
            uid: None,
            start,
            end,
            display: vec![mkv::ChapterDisplay {
                name: name.to_owned(),
                language: None,
            }],
            hidden,
            children: Vec::new(),
        };
        mkv::Edition {
            uid: None,
            hidden: false,
            ordered: matches!(order, EditionOrder::Ordered),
            chapters: vec![
                chapter("Opening", 10_000_000_000, 90_000_000_000, false),
                chapter("Credits", 90_000_000_000, 100_000_000_000, true),
            ],
        }
    }

    fn patterns() -> SkipPatterns {
        SkipPatterns::new(
            &["^OP$".to_owned(), "Opening".to_owned(), "Intro".to_owned()],
            &["^ED$".to_owned(), "Ending".to_owned(), "Credits".to_owned()],
        )
    }

    #[test]
    fn chapter_names_match_the_default_patterns_case_insensitively() {
        let patterns = patterns();
        let cases = [
            ("OP", Some(ChapterKind::Opening)),
            ("op", Some(ChapterKind::Opening)),
            ("Opening", Some(ChapterKind::Opening)),
            ("opening 2", Some(ChapterKind::Opening)),
            ("INTRO", Some(ChapterKind::Opening)),
            ("ED", Some(ChapterKind::Ending)),
            ("ed", Some(ChapterKind::Ending)),
            ("Ending", Some(ChapterKind::Ending)),
            ("Credits", Some(ChapterKind::Ending)),
            ("OP 1", None),
            ("ED 1", None),
            ("Recap", None),
            ("", None),
        ];
        for (title, expected) in cases {
            assert_eq!(patterns.classify(Some(title)), expected, "{title:?}");
        }
        assert_eq!(
            patterns.classify(None),
            None,
            "an untitled chapter is not a skip"
        );
    }

    #[test]
    fn an_invalid_pattern_does_not_disable_the_others() {
        let patterns = SkipPatterns::new(
            &["[".to_owned(), "Opening".to_owned()],
            &["Ending".to_owned()],
        );
        assert_eq!(
            patterns.classify(Some("Opening")),
            Some(ChapterKind::Opening)
        );
        assert_eq!(patterns.classify(Some("Ending")), Some(ChapterKind::Ending));
    }

    #[test]
    fn ordered_editions_fall_back_to_the_player_chapter_list() {
        assert!(
            from_edition(&edition(EditionOrder::Ordered), &patterns()).is_empty(),
            "ordered chapter times do not match mpv's timeline"
        );
    }

    #[test]
    fn hidden_chapters_are_not_skip_markers() {
        let chapters = from_edition(&edition(EditionOrder::Unordered), &patterns());
        assert_eq!(chapters.len(), 1, "the hidden Credits chapter is dropped");
        assert_eq!(
            chapters.first().and_then(|chapter| chapter.kind),
            Some(ChapterKind::Opening)
        );
    }

    #[test]
    fn player_chapters_fall_back_when_the_container_has_none() {
        let patterns = patterns();
        let list = [
            player::Chapter {
                title: Some("Opening".to_owned()),
                start: 10.0,
            },
            player::Chapter {
                title: Some("Ending".to_owned()),
                start: 100.0,
            },
        ];
        let fallback = from_player(&list, &patterns);
        assert_eq!(fallback.len(), 2);
        let first = fallback.first().expect("the opening chapter");
        assert_eq!(first.start, Duration::from_secs(10));
        assert_eq!(first.kind, Some(ChapterKind::Opening));
        assert_eq!(
            fallback.get(1).and_then(|chapter| chapter.kind),
            Some(ChapterKind::Ending)
        );

        let container = list.iter().take(1).cloned().collect::<Vec<_>>();
        let from_mkv = from_player(&container, &patterns);
        assert_eq!(prefer_mkv(Vec::new(), fallback.clone()), fallback);
        assert_eq!(prefer_mkv(from_mkv.clone(), fallback), from_mkv);
    }

    #[test]
    fn a_chapter_without_an_end_runs_to_the_next_one() {
        let list = [player::Chapter {
            title: Some("Opening".to_owned()),
            start: 10.0,
        }];
        let chapters = with_ends(
            from_player(&list, &patterns()),
            Some(Duration::from_secs(90)),
        );
        assert_eq!(
            chapters.first().and_then(|chapter| chapter.end),
            Some(Duration::from_secs(90))
        );
    }
}
