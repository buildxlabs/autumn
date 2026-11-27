//! Title normalization used by the matcher.
//!
//! A normalized title keeps the words a release name actually says, with
//! season and year markers extracted and romanization and punctuation folded
//! away. `base` drops format words and roman numerals so sequels still share
//! a root title.
//!
//! Ported from seanime's `internal/library/scanner/title_normalization.go`,
//! GPL-3.0. Original copyright stays with the seanime authors; the license is
//! in `reference/seanime/LICENSE`.

const NOISE: &[&str] = &[
    "the",
    "a",
    "an",
    "of",
    "to",
    "in",
    "for",
    "on",
    "with",
    "at",
    "by",
    "from",
    "as",
    "is",
    "it",
    "that",
    "this",
    "be",
    "are",
    "was",
    "were",
    "no",
    "wa",
    "wo",
    "ga",
    "ni",
    "de",
    "ka",
    "mo",
    "ya",
    "e",
    "he",
    "anime",
    "animation",
    "tv",
    "movie",
    "film",
];

const FORMAT: &[&str] = &[
    "ova",
    "ona",
    "oad",
    "oav",
    "sp",
    "special",
    "specials",
    "movie",
    "film",
    "tv",
    "nc",
    "nced",
    "ncop",
    "extras",
    "extra",
    "opening",
    "ending",
    "preview",
    "finale",
    "uncensored",
    "censored",
    "bluray",
];

const ROMAN: &[(&str, u32)] = &[
    ("ii", 2),
    ("iii", 3),
    ("iv", 4),
    ("v", 5),
    ("vi", 6),
    ("vii", 7),
    ("viii", 8),
    ("ix", 9),
    ("xi", 11),
    ("xii", 12),
    ("xiii", 13),
];

const ORDINALS: &[(&str, u32)] = &[
    ("first", 1),
    ("1st", 1),
    ("second", 2),
    ("2nd", 2),
    ("third", 3),
    ("3rd", 3),
    ("fourth", 4),
    ("4th", 4),
    ("fifth", 5),
    ("5th", 5),
];

/// A title folded for matching, with its extracted markers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizedTitle {
    /// Words with punctuation, season and year removed.
    pub key: String,
    /// `key` without format words, roman numerals and a trailing number.
    pub base: String,
    /// Significant words of `key`, with noise words removed.
    pub tokens: Vec<String>,
    /// The season the title states, if any.
    pub season: Option<u32>,
    /// The year the title states, if any.
    pub year: Option<u32>,
}

/// Folds a title into a [`NormalizedTitle`].
#[must_use]
pub fn normalize(title: &str) -> NormalizedTitle {
    let folded = title.to_lowercase().replace('ō', "ou").replace('ū', "uu");
    let mut words: Vec<String> = folded
        .split(|c: char| !c.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_owned)
        .collect();

    let mut season = None;
    let mut year = None;
    let mut i = 0;
    while i < words.len() {
        let Some(word) = words.get(i).map(String::as_str) else {
            break;
        };
        if word.len() == 4
            && word.bytes().all(|b| b.is_ascii_digit())
            && let Ok(value) = word.parse::<u32>()
            && (1900..=2100).contains(&value)
        {
            year = Some(value);
            words.remove(i);
            continue;
        }
        if let Some(value) = short_season(word) {
            season = Some(value);
            words.remove(i);
            continue;
        }
        if word == "season"
            && let Some(previous) = i.checked_sub(1).and_then(|before| words.get(before))
            && let Some(value) = word_number(previous)
        {
            season = Some(value);
            words.remove(i);
            words.remove(i - 1);
            i -= 1;
            continue;
        }
        if word == "season"
            && let Some(value) = words.get(i + 1).and_then(|next| next.parse::<u32>().ok())
        {
            season = Some(value);
            words.remove(i + 1);
            words.remove(i);
            continue;
        }
        if let Some(value) = ordinal(word)
            && words.get(i + 1).is_some_and(|next| next == "season")
        {
            season = Some(value);
            words.remove(i + 1);
            words.remove(i);
            continue;
        }
        i += 1;
    }

    let key = words.join(" ");
    let mut base_words = words.clone();
    if base_words
        .last()
        .is_some_and(|word| word.bytes().all(|b| b.is_ascii_digit()))
    {
        base_words.pop();
    }
    let base = base_words
        .iter()
        .filter(|word| !FORMAT.contains(&word.as_str()) && roman(word).is_none())
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    let tokens = words
        .iter()
        .filter(|word| word.len() > 1 && !NOISE.contains(&word.as_str()))
        .cloned()
        .collect();

    NormalizedTitle {
        key,
        base,
        tokens,
        season,
        year,
    }
}

/// `S02`, `s2` and Japanese `2期`.
fn short_season(word: &str) -> Option<u32> {
    if let Some(rest) = word.strip_prefix('s')
        && !rest.is_empty()
        && rest.bytes().all(|b| b.is_ascii_digit())
    {
        return rest.parse().ok();
    }
    let digits = word.strip_suffix('期')?;
    digits.parse().ok()
}

fn word_number(word: &str) -> Option<u32> {
    word.parse::<u32>().ok().or_else(|| ordinal(word))
}

fn ordinal(word: &str) -> Option<u32> {
    ORDINALS
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, number)| *number)
}

fn roman(word: &str) -> Option<u32> {
    ROMAN
        .iter()
        .find(|(name, _)| *name == word)
        .map(|(_, number)| *number)
}

#[cfg(test)]
mod tests {
    use super::normalize;

    #[test]
    fn folds_case_punctuation_and_macrons() {
        let got = normalize("Kōkoro ga Sakebitagatterunda!");
        assert_eq!(
            got.key, "koukoro ga sakebitagatterunda",
            "macron becomes a double vowel and punctuation separates words"
        );
    }

    #[test]
    fn normalization_table() {
        let cases = [
            (
                "Fullmetal Alchemist: Brotherhood (2009)",
                "fullmetal alchemist brotherhood",
                "fullmetal alchemist brotherhood",
                None,
                Some(2009),
            ),
            (
                "Sword Art Online II",
                "sword art online ii",
                "sword art online",
                None,
                None,
            ),
            (
                "Sword Art Online 2nd Season",
                "sword art online",
                "sword art online",
                Some(2),
                None,
            ),
            (
                "Spy x Family Season 2",
                "spy x family",
                "spy x family",
                Some(2),
                None,
            ),
            (
                "Kimi no Na wa. S02",
                "kimi no na wa",
                "kimi no na wa",
                Some(2),
                None,
            ),
            (
                "Shingeki no Kyojin 2期",
                "shingeki no kyojin",
                "shingeki no kyojin",
                Some(2),
                None,
            ),
        ];
        for (title, key, base, season, year) in cases {
            let got = normalize(title);
            assert_eq!(got.key, key, "key of {title:?}");
            assert_eq!(got.base, base, "base of {title:?}");
            assert_eq!(got.season, season, "season of {title:?}");
            assert_eq!(got.year, year, "year of {title:?}");
        }
    }

    #[test]
    fn significant_tokens_drop_noise_words() {
        let got = normalize("The Animation of Sword Art Online");
        assert_eq!(got.tokens, ["sword", "art", "online"]);
    }
}
