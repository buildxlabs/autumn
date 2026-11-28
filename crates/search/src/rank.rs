//! Auto-select filtering and ranking with an `AutoSelectProfile` (#181 Q8–Q9).
//!
//! Hard filters run first: the episode match (Q5–Q7), excluded terms,
//! minimum seeders, size bounds, the `Only`/`Never` preferences and the
//! require flags. A filter with a bound set never lets an unknown value
//! through. Survivors are ordered by each criterion in turn, an earlier one
//! always outranking a later one: resolution, release group, codec,
//! seeders, size, dual audio, batch, then the optional language, source and
//! multiple-subs preferences. Ties go to the highest version, then a
//! non-remake, then the lowest infohash.

use std::cmp::{Ordering, Reverse};

use release_parser::{Audio, Release, Subtitle, VideoCodec, parse};
use settings::{AutoSelectProfile, Codec, Preference, Source};

use crate::{EpisodeTarget, Torrent};

/// Filter `torrents` to the releases of `target` that pass `profile`, best first.
#[must_use]
pub fn rank(
    torrents: Vec<Torrent>,
    target: &EpisodeTarget,
    profile: &AutoSelectProfile,
) -> Vec<Torrent> {
    let mut kept: Vec<Candidate> = torrents
        .into_iter()
        .map(Candidate::new)
        .filter(|candidate| candidate.rejection(target, profile).is_none())
        .collect();
    kept.sort_by(|a, b| a.compare(b, profile));
    kept.into_iter()
        .map(|candidate| candidate.torrent)
        .collect()
}

/// One result scored against a profile: passing results are ranked first,
/// failing ones are kept (dimmed) with their hard-filter reason (#182 Q5).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Ranked {
    /// The result.
    pub torrent: Torrent,
    /// Whether it passes every hard filter.
    pub passes: bool,
    /// The first hard-filter failure, `None` when it passes. `min seeders`,
    /// `excluded term`, `size`, `not an episode`, `wrong season`,
    /// `wrong episode`, `only`, `never`, `require …`.
    pub status: Option<String>,
}

/// Every result, passing (ranked, best first) then failing, each with its
/// status. Failing results keep input order among themselves (#182 Q5).
#[must_use]
pub fn rank_with_status(
    torrents: Vec<Torrent>,
    target: &EpisodeTarget,
    profile: &AutoSelectProfile,
) -> Vec<Ranked> {
    let mut passing: Vec<Candidate> = Vec::new();
    let mut failing: Vec<Ranked> = Vec::new();
    for torrent in torrents {
        let candidate = Candidate::new(torrent);
        match candidate.rejection(target, profile) {
            None => passing.push(candidate),
            Some(reason) => failing.push(Ranked {
                torrent: candidate.torrent,
                passes: false,
                status: Some(reason.to_owned()),
            }),
        }
    }
    passing.sort_by(|a, b| a.compare(b, profile));
    let mut ranked: Vec<Ranked> = passing
        .into_iter()
        .map(|candidate| Ranked {
            torrent: candidate.torrent,
            passes: true,
            status: None,
        })
        .collect();
    ranked.extend(failing);
    ranked
}

/// A result with its release name parsed once.
struct Candidate {
    torrent: Torrent,
    release: Release,
    lower_name: String,
}

impl Candidate {
    fn new(torrent: Torrent) -> Self {
        let mut release = parse(&torrent.name);
        release.batch |= torrent.is_batch;
        let lower_name = torrent.name.to_lowercase();
        Self {
            torrent,
            release,
            lower_name,
        }
    }

    fn rejection(
        &self,
        target: &EpisodeTarget,
        profile: &AutoSelectProfile,
    ) -> Option<&'static str> {
        // Whether a preference lets a release with or without the property through.
        let allows = |preference: Preference, has: bool| match preference {
            Preference::Only => has,
            Preference::Never => !has,
            Preference::Neutral | Preference::Prefer | Preference::Avoid => true,
        };
        let seeders = self.torrent.seeders.unwrap_or(0);
        let size_ok = |bound: Option<u64>, ok: fn(u64, u64) -> bool| {
            bound.is_none_or(|bound| self.torrent.size.is_some_and(|size| ok(size, bound)))
        };
        if let Some(reason) = crate::matching::match_rejection(&self.release, target) {
            return Some(reason);
        }
        if profile
            .excluded_terms
            .iter()
            .map(|term| term.trim().to_lowercase())
            .any(|term| !term.is_empty() && self.lower_name.contains(&term))
        {
            return Some("excluded term");
        }
        if seeders < profile.min_seeders {
            return Some("min seeders");
        }
        if !size_ok(profile.min_size, |size, min| size >= min)
            || !size_ok(profile.max_size, |size, max| size <= max)
        {
            return Some("size");
        }
        if !allows(profile.multiple_audio, self.multiple_audio()) {
            return Some(if self.multiple_audio() {
                "never"
            } else {
                "only"
            });
        }
        if !allows(profile.batch, self.release.batch) {
            return Some(if self.release.batch { "never" } else { "only" });
        }
        if !allows(profile.multiple_subs, self.multiple_subs()) {
            return Some(if self.multiple_subs() {
                "never"
            } else {
                "only"
            });
        }
        if profile.require_codec && position(&profile.codecs, self.codec()).is_none() {
            return Some("require codec");
        }
        if profile.require_source && position(&profile.sources, self.source()).is_none() {
            return Some("require source");
        }
        if profile.require_language && self.language_rank(profile).is_none() {
            return Some("require language");
        }
        None
    }

    fn compare(&self, other: &Self, profile: &AutoSelectProfile) -> Ordering {
        // Order two releases by a prefer/avoid preference; other values tie.
        let leaning = |preference: Preference, a: bool, b: bool| match preference {
            Preference::Prefer => b.cmp(&a),
            Preference::Avoid => a.cmp(&b),
            Preference::Neutral | Preference::Only | Preference::Never => Ordering::Equal,
        };
        let resolution = |c: &Self| position(&profile.resolutions, c.release.resolution);
        let group = |c: &Self| c.group_rank(profile);
        let codec = |c: &Self| position(&profile.codecs, c.codec());
        let language = |c: &Self| c.language_rank(profile);
        let source = |c: &Self| position(&profile.sources, c.source());
        listed(resolution(self), resolution(other))
            .then_with(|| listed(group(self), group(other)))
            .then_with(|| listed(codec(self), codec(other)))
            .then_with(|| other.torrent.seeders.cmp(&self.torrent.seeders))
            .then_with(|| listed(self.torrent.size, other.torrent.size))
            .then_with(|| {
                leaning(
                    profile.multiple_audio,
                    self.multiple_audio(),
                    other.multiple_audio(),
                )
            })
            .then_with(|| leaning(profile.batch, self.release.batch, other.release.batch))
            .then_with(|| listed(language(self), language(other)))
            .then_with(|| listed(source(self), source(other)))
            .then_with(|| {
                leaning(
                    profile.multiple_subs,
                    self.multiple_subs(),
                    other.multiple_subs(),
                )
            })
            .then_with(|| version(other).cmp(&version(self)))
            .then_with(|| self.torrent.remake.cmp(&other.torrent.remake))
            .then_with(|| self.torrent.infohash.cmp(&other.torrent.infohash))
    }

    fn multiple_audio(&self) -> bool {
        self.release
            .audio
            .iter()
            .any(|audio| matches!(audio, Audio::DualAudio | Audio::MultiAudio))
    }

    fn multiple_subs(&self) -> bool {
        self.release.subtitles.contains(&Subtitle::Multi)
    }

    fn codec(&self) -> Option<Codec> {
        Some(match self.release.video_codec? {
            VideoCodec::H264 => Codec::H264,
            VideoCodec::H265 => Codec::H265,
            VideoCodec::Av1 => Codec::Av1,
            VideoCodec::Xvid => Codec::Xvid,
            VideoCodec::Vp9 => Codec::Vp9,
            VideoCodec::Mpeg2 => Codec::Mpeg2,
            _ => return None,
        })
    }

    fn source(&self) -> Option<Source> {
        use release_parser::Source as Parsed;
        Some(match self.release.source? {
            Parsed::BluRay => Source::BluRay,
            Parsed::BdRip => Source::BdRip,
            Parsed::WebDl => Source::WebDl,
            Parsed::WebRip => Source::WebRip,
            Parsed::Dvd => Source::Dvd,
            Parsed::DvdRip => Source::DvdRip,
            Parsed::Tv => Source::Tv,
            _ => return None,
        })
    }

    fn group_rank(&self, profile: &AutoSelectProfile) -> Option<usize> {
        let group = self.release.group.as_deref()?;
        profile
            .release_groups
            .iter()
            .position(|wanted| wanted.trim().eq_ignore_ascii_case(group))
    }

    fn language_rank(&self, profile: &AutoSelectProfile) -> Option<usize> {
        profile
            .preferred_languages
            .iter()
            .position(|language| contains_word(&self.lower_name, &language.trim().to_lowercase()))
    }
}

/// Lower is better, and `None` (unknown or unlisted) comes last.
fn listed<T: Ord>(a: Option<T>, b: Option<T>) -> Ordering {
    a.map(Reverse).cmp(&b.map(Reverse)).reverse()
}

fn position<T: PartialEq>(list: &[T], value: Option<T>) -> Option<usize> {
    let value = value?;
    list.iter().position(|item| *item == value)
}

/// A release with no `vK` marker is version 1.
fn version(candidate: &Candidate) -> u32 {
    candidate.release.version.unwrap_or(1)
}

/// Whether `word` occurs in `text` bounded by non-alphanumeric characters.
fn contains_word(text: &str, word: &str) -> bool {
    !word.is_empty()
        && text.match_indices(word).any(|(start, _)| {
            let end = start + word.len();
            let before = text.get(..start).and_then(|s| s.chars().next_back());
            let after = text.get(end..).and_then(|s| s.chars().next());
            !before.is_some_and(char::is_alphanumeric) && !after.is_some_and(char::is_alphanumeric)
        })
}
