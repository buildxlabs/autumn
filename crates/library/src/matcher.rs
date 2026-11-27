//! Threshold matcher for a group of local files.
//!
//! A title is normalized once and compared with every candidate's titles.
//! The score folds title similarity, significant-token overlap and season and
//! year agreement into one value between zero and one; only a candidate above
//! the threshold wins. There is no legacy fallback (OD-4).

use std::collections::HashSet;

use domain::ExternalId;

use crate::normalize::{NormalizedTitle, normalize};

/// The default match threshold, from seanime (OD-4).
pub const DEFAULT_THRESHOLD: f64 = 0.5;

/// A media the caller considers as a match: a title, aliases and the external
/// ids to cross-reference when it wins (ADR 0001).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    /// The catalog title.
    pub title: String,
    /// Other titles the media is known by.
    pub aliases: Vec<String>,
    /// Every external id known for the media.
    pub external_ids: Vec<ExternalId>,
    /// AniList ids of the media's sequels, for out-of-range files.
    sequels: Vec<u32>,
}

impl Candidate {
    /// A candidate with no aliases.
    pub fn new(title: impl Into<String>, external_ids: Vec<ExternalId>) -> Self {
        Self {
            title: title.into(),
            aliases: Vec::new(),
            external_ids,
            sequels: Vec::new(),
        }
    }

    /// Adds another title the media is known by.
    #[must_use]
    pub fn with_alias(mut self, alias: impl Into<String>) -> Self {
        self.aliases.push(alias.into());
        self
    }

    /// Names a sequel by its AniList id, for files beyond this media's run.
    #[must_use]
    pub fn with_sequel(mut self, anilist_id: u32) -> Self {
        self.sequels.push(anilist_id);
        self
    }

    /// The AniList ids of this media's sequels.
    #[must_use]
    pub fn sequels(&self) -> &[u32] {
        &self.sequels
    }
}

/// Which candidate won a group, and with what confidence.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Match {
    /// The index of the winning candidate in the slice passed to [`Matcher`].
    pub candidate: usize,
    /// The score that won, between zero and one.
    pub confidence: f64,
}

/// Matches normalized titles against a caller-supplied candidate list.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matcher {
    threshold: f64,
}

impl Default for Matcher {
    fn default() -> Self {
        Self::new(DEFAULT_THRESHOLD)
    }
}

impl Matcher {
    /// A matcher that only accepts scores strictly above `threshold`.
    pub const fn new(threshold: f64) -> Self {
        Self { threshold }
    }

    /// The score of the best of a candidate's titles.
    #[must_use]
    pub fn score(&self, title: &NormalizedTitle, candidate: &Candidate) -> f64 {
        std::iter::once(&candidate.title)
            .chain(candidate.aliases.iter())
            .map(|alias| title_score(title, &normalize(alias)))
            .fold(0.0_f64, f64::max)
    }

    /// The best candidate above the threshold, if there is one.
    #[must_use]
    pub fn best(&self, title: &NormalizedTitle, candidates: &[Candidate]) -> Option<Match> {
        self.best_candidate(title, candidates)
            .filter(|found| found.confidence > self.threshold)
    }

    /// The best candidate that scored above zero, regardless of the threshold.
    /// A scan uses this to report why a file was left unmatched.
    #[must_use]
    pub fn best_candidate(
        &self,
        title: &NormalizedTitle,
        candidates: &[Candidate],
    ) -> Option<Match> {
        candidates
            .iter()
            .enumerate()
            .map(|(index, candidate)| (index, self.score(title, candidate)))
            .filter(|(_, score)| *score > 0.0)
            // `max_by` keeps the last of equal scores; the caller lists its
            // preferred candidates first, so the first one must win a tie.
            .reduce(|best, next| if next.1 > best.1 { next } else { best })
            .map(|(candidate, confidence)| Match {
                candidate,
                confidence,
            })
    }
}

fn title_score(file: &NormalizedTitle, media: &NormalizedTitle) -> f64 {
    let key = strsim::jaro_winkler(&file.key, &media.key);
    let base = strsim::jaro_winkler(&file.base, &media.base);
    let tokens = token_ratio(&file.tokens, &media.tokens);
    let mut score = 0.5 * key + 0.3 * tokens + 0.2 * base;
    match (file.season, media.season) {
        (Some(a), Some(b)) => score += if a == b { 0.05 } else { -0.25 },
        // A name without a season marker is a first season, so it should not
        // tie with a title that names a later one.
        (None, Some(b)) if b > 1 => score -= 0.05,
        _ => {}
    }
    if let (Some(a), Some(b)) = (file.year, media.year) {
        score += if a.abs_diff(b) <= 1 { 0.02 } else { -0.1 };
    }
    score.clamp(0.0, 1.0)
}

fn token_ratio(a: &[String], b: &[String]) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    let right: HashSet<&str> = b.iter().map(String::as_str).collect();
    let matched = a
        .iter()
        .filter(|token| right.contains(token.as_str()))
        .count();
    let matched = f64::from(u32::try_from(matched).unwrap_or(u32::MAX));
    let total = f64::from(u32::try_from(usize::min(a.len(), b.len())).unwrap_or(u32::MAX));
    matched / total
}

#[cfg(test)]
mod tests {
    use domain::ExternalId;

    use super::{Candidate, DEFAULT_THRESHOLD, Matcher};
    use crate::normalize::normalize;

    fn candidate(title: &str) -> Candidate {
        Candidate::new(title, vec![ExternalId::AniList(1)])
    }

    #[test]
    fn an_exact_title_wins_at_the_default_threshold() {
        let matcher = Matcher::default();
        let title = normalize("Fullmetal Alchemist Brotherhood");
        let found = matcher.best(&title, &[candidate("Fullmetal Alchemist Brotherhood")]);
        assert!(
            found.is_some(),
            "an exact title is above {DEFAULT_THRESHOLD}"
        );
        assert!((found.unwrap().confidence - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn threshold_is_strict_just_below_and_just_above() {
        let title = normalize("Series Alpha");
        let candidates = [candidate("Series Alpha")];
        let score = Matcher::default().score(&title, &candidates[0]);
        assert!(score > f64::from(0) && score <= 1.0, "score {score}");

        let below = Matcher::new(score - f64::EPSILON).best(&title, &candidates);
        assert!(below.is_some(), "just below the score still matches");
        let at = Matcher::new(score).best(&title, &candidates);
        assert_eq!(at, None, "a score equal to the threshold does not match");
        let above = Matcher::new(score + f64::EPSILON).best(&title, &candidates);
        assert_eq!(above, None, "a threshold just above the score rejects");
    }

    #[test]
    fn threshold_table() {
        let title = normalize("Series Alpha");
        let candidates = [
            candidate("Series Alpha"),
            candidate("Completely Different Show"),
        ];
        let cases = [(0.0, Some(0)), (DEFAULT_THRESHOLD, Some(0)), (1.0, None)];
        for (threshold, expected) in cases {
            let found = Matcher::new(threshold).best(&title, &candidates);
            assert_eq!(
                found.map(|matched| matched.candidate),
                expected,
                "threshold {threshold}"
            );
        }
    }

    #[test]
    fn a_tied_score_goes_to_the_first_candidate() {
        let title = normalize("Series Alpha");
        let candidates = [
            Candidate::new("Series Alpha", vec![ExternalId::AniList(1)]),
            Candidate::new("Series Alpha", vec![ExternalId::AniList(2)]),
        ];
        let found = Matcher::default().best(&title, &candidates);
        assert_eq!(found.map(|matched| matched.candidate), Some(0));
    }

    #[test]
    fn aliases_can_win_and_disagreeing_seasons_are_penalized() {
        let title = normalize("Fullmetal Alchemist: Brotherhood S02");
        let candidates = [
            candidate("Unrelated Show").with_alias("Fullmetal Alchemist Brotherhood 2nd Season"),
            candidate("Fullmetal Alchemist Brotherhood Season 3"),
        ];
        let found = Matcher::default().best(&title, &candidates);
        assert_eq!(found.map(|matched| matched.candidate), Some(0));
    }
}
