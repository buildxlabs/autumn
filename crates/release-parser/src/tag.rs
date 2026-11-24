//! Recognizers for single words and bracket contents.

use crate::EpisodeKind;

/// Video containers a release file can end in.
const EXTENSIONS: &[&str] = &[
    "avi", "flv", "m2ts", "m4v", "mkv", "mov", "mp4", "ogm", "rmvb", "ts", "webm", "wmv",
];

/// Source, codec, bit depth, language and edition tags, lowercase.
const TECH: &[&str] = &[
    "10-bit",
    "10bit",
    "8-bit",
    "8bit",
    "adn",
    "amzn",
    "av1",
    "avc",
    "avc-8bit",
    "bd",
    "bdmv",
    "bdremux",
    "bdrip",
    "bili",
    "bilibili",
    "blu-ray",
    "bluray",
    "cr",
    "dsnp",
    "dual",
    "dual-audio",
    "dub",
    "dvd",
    "dvdrip",
    "h.264",
    "h.265",
    "h264",
    "h265",
    "hdtv",
    "hevc",
    "hevc-10bit",
    "hi10",
    "hi10p",
    "hulu",
    "msubs",
    "multi",
    "multi-sub",
    "multi-subs",
    "multisub",
    "nf",
    "remux",
    "subfrench",
    "tvrip",
    "uncensored",
    "vostfr",
    "web",
    "web-dl",
    "web-rip",
    "webdl",
    "webrip",
    "x264",
    "x265",
    "xvid",
];

/// Audio codecs, which often carry a channel layout (`AAC2.0`, `DDP5.1`).
const AUDIO: &[&str] = &[
    "aac", "ac3", "dd", "ddp", "dts", "e-ac-3", "eac3", "flac", "opus",
];

/// Words that name an episode kind other than a regular episode, lowercase.
/// Longer words come before their prefixes (`specials` before `sp`).
const KINDS: &[(&str, EpisodeKind)] = &[
    ("ncop", EpisodeKind::Ncop),
    ("nced", EpisodeKind::Nced),
    ("opening", EpisodeKind::Ncop),
    ("ending", EpisodeKind::Nced),
    ("op", EpisodeKind::Ncop),
    ("ed", EpisodeKind::Nced),
    ("specials", EpisodeKind::Special),
    ("special", EpisodeKind::Special),
    ("sp", EpisodeKind::Special),
    ("ova", EpisodeKind::Special),
    ("oad", EpisodeKind::Special),
    ("ona", EpisodeKind::Special),
    ("movie", EpisodeKind::Movie),
    ("preview", EpisodeKind::Extra),
    ("menu", EpisodeKind::Extra),
    ("pv", EpisodeKind::Extra),
    ("cm", EpisodeKind::Extra),
];

/// A number with an optional version: `01` or `01v2`.
pub(crate) type Numbered = (u32, Option<u32>);

/// Splits a known video extension off `name`, lowercased.
pub(crate) fn split_extension(name: &str) -> (&str, Option<String>) {
    if let Some((stem, extension)) = name.rsplit_once('.') {
        let extension = extension.to_ascii_lowercase();
        if EXTENSIONS.contains(&extension.as_str()) {
            return (stem, Some(extension));
        }
    }
    (name, None)
}

/// Splits the leading ASCII digits off `s`.
fn split_digits(s: &str) -> (&str, &str) {
    let end = s.find(|c: char| !c.is_ascii_digit()).unwrap_or(s.len());
    s.split_at_checked(end).unwrap_or((s, ""))
}

fn digits_only(s: &str) -> Option<u32> {
    let (digits, rest) = split_digits(s);
    if rest.is_empty() {
        digits.parse().ok()
    } else {
        None
    }
}

/// Splits a leading `v2` off `s`.
fn split_version(s: &str) -> (Option<u32>, &str) {
    if let Some(after) = s.strip_prefix(['v', 'V']) {
        let (digits, rest) = split_digits(after);
        if let Ok(version) = digits.parse() {
            return (Some(version), rest);
        }
    }
    (None, s)
}

/// `01`, `1000` or `01v2`.
pub(crate) fn number(word: &str) -> Option<Numbered> {
    let (digits, rest) = split_digits(word);
    let number = digits.parse().ok()?;
    let (version, rest) = split_version(rest);
    rest.is_empty().then_some((number, version))
}

/// A four-digit year from 1900 to 2099.
pub(crate) fn year(word: &str) -> Option<u32> {
    let year = digits_only(word).filter(|_| word.len() == 4)?;
    (1900..=2099).contains(&year).then_some(year)
}

/// The vertical resolution in `1080p`, `1920x1080` or `4K`.
pub(crate) fn resolution(word: &str) -> Option<u32> {
    let lower = word.to_ascii_lowercase();
    let lines = |s: &str| digits_only(s).filter(|_| (3..=4).contains(&s.len()));
    if lower == "4k" {
        return Some(2160);
    }
    if let Some(height) = lower.strip_suffix('p') {
        return lines(height);
    }
    let (width, height) = lower.split_once(['x', '×'])?;
    lines(width).and(lines(height))
}

/// An eight-digit hexadecimal CRC32, as release groups put it in brackets.
pub(crate) fn checksum(content: &str) -> Option<u32> {
    if content.len() != 8 || !content.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    u32::from_str_radix(content, 16).ok()
}

/// A technical tag or resolution, which ends the title and episode title.
pub(crate) fn is_tech(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let audio = lower.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.');
    resolution(&lower).is_some() || TECH.contains(&lower.as_str()) || AUDIO.contains(&audio)
}

/// Tags that are also ordinary words (`DD Hokuto no Ken`, `Magnum Opus`). They
/// count only after the episode marker, never inside the title.
const AMBIGUOUS_TAGS: &[&str] = &[
    "adn", "bd", "cr", "dd", "dual", "dub", "multi", "nf", "opus", "web",
];

/// A technical tag that can end the title: [`is_tech`] minus the tags that are
/// also ordinary words.
pub(crate) fn ends_title(word: &str) -> bool {
    is_tech(word) && !AMBIGUOUS_TAGS.contains(&word.to_ascii_lowercase().as_str())
}

/// A kind word that is also an ordinary English word (`Happy Ending`), so it
/// needs more context than an acronym such as `OVA` to mark the kind.
pub(crate) fn is_dictionary_kind(word: &str) -> bool {
    const WORDS: [&str; 7] = [
        "ending", "menu", "movie", "opening", "preview", "special", "specials",
    ];
    WORDS.contains(&word.to_ascii_lowercase().as_str())
}

/// A version on its own: `v2`, `V3`.
pub(crate) fn version_word(word: &str) -> Option<u32> {
    digits_only(word.strip_prefix(['v', 'V'])?)
}

/// `S2`, `S02` or `Season2`.
pub(crate) fn season(word: &str) -> Option<u32> {
    let lower = word.to_ascii_lowercase();
    let digits = lower
        .strip_prefix("season")
        .or_else(|| lower.strip_prefix('s'))?;
    digits_only(digits)
}

/// `2nd` or `Second`, as in `2nd Season`.
pub(crate) fn ordinal(word: &str) -> Option<u32> {
    const WORDS: [&str; 6] = ["first", "second", "third", "fourth", "fifth", "sixth"];
    let lower = word.to_ascii_lowercase();
    if let Some(position) = WORDS.iter().position(|w| *w == lower) {
        return u32::try_from(position + 1).ok();
    }
    let (digits, suffix) = split_digits(&lower);
    let suffixed = ["st", "nd", "rd", "th"].contains(&suffix);
    if suffixed { digits.parse().ok() } else { None }
}

/// A season-and-episode word: `S01E05`, `S01E05v2`, `S03E01-Title` or `S01OVA01`.
#[derive(Debug)]
pub(crate) struct SeasonEpisode<'w> {
    pub(crate) season: u32,
    pub(crate) episode: Option<Numbered>,
    pub(crate) kind: Option<(EpisodeKind, Option<Numbered>)>,
    /// The end of a range such as `S01E14-E26`.
    pub(crate) last_episode: Option<u32>,
    /// Text glued on after a `-`, the start of the episode title.
    pub(crate) rest: &'w str,
}

pub(crate) fn season_episode(word: &str) -> Option<SeasonEpisode<'_>> {
    let after_s = word.strip_prefix(['S', 's'])?;
    let (digits, after_season) = split_digits(after_s);
    let season = digits.parse().ok()?;
    if let Some(after_e) = after_season.strip_prefix(['E', 'e']) {
        let (digits, after_episode) = split_digits(after_e);
        let episode = digits.parse().ok()?;
        let (version, rest) = split_version(after_episode);
        let rest = if rest.is_empty() {
            rest
        } else {
            rest.strip_prefix('-')?
        };
        // `S01E14-E26`, `S01E14-26` or `S02E01-S02E12`: a range, not an episode title.
        let range_end = rest
            .strip_prefix(['S', 's'])
            .map(|after| split_digits(after).1)
            .filter(|after| after.starts_with(['E', 'e']))
            .unwrap_or(rest);
        let last_episode = digits_only(range_end.strip_prefix(['E', 'e']).unwrap_or(range_end))
            .filter(|last| *last > episode);
        return Some(SeasonEpisode {
            season,
            episode: Some((episode, version)),
            kind: None,
            last_episode,
            rest: if last_episode.is_some() { "" } else { rest },
        });
    }
    let kind = kind_word(after_season).filter(|(_, number)| number.is_some())?;
    Some(SeasonEpisode {
        season,
        episode: None,
        kind: Some(kind),
        last_episode: None,
        rest: "",
    })
}

/// An episode word: `E01`, `EP01`, `Episode01`, `episode-01`, or a bare `EP`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EpisodePrefix {
    /// The number is in the word.
    Numbered(Numbered),
    /// A bare `EP`, `Ep` or `Episode`: the number is the next word.
    NumberFollows,
}

pub(crate) fn episode_prefix(word: &str) -> Option<EpisodePrefix> {
    let lower = word.to_ascii_lowercase();
    for prefix in ["episode", "ep", "e"] {
        let Some(rest) = lower.strip_prefix(prefix) else {
            continue;
        };
        if rest.is_empty() && prefix != "e" {
            return Some(EpisodePrefix::NumberFollows);
        }
        if let Some(numbered) = number(rest.strip_prefix('-').unwrap_or(rest)) {
            return Some(EpisodePrefix::Numbered(numbered));
        }
    }
    None
}

/// An episode range: `01-12`, `01~12` or `E01-E12`. A year range such as
/// `2019-2020` is not one.
pub(crate) fn episode_range(word: &str) -> Option<(u32, u32)> {
    let (first, last) = word.split_once(['-', '~'])?;
    let episode = |s: &str| {
        let s = s.strip_prefix(['E', 'e']).unwrap_or(s);
        digits_only(s.strip_prefix(['P', 'p']).unwrap_or(s))
    };
    let (first_number, last_number) = (episode(first)?, episode(last)?);
    let years = year(first).is_some() && year(last).is_some();
    (first_number < last_number && !years).then_some((first_number, last_number))
}

/// A season range: `S01-S03` or `S1-3`.
pub(crate) fn season_range(word: &str) -> Option<(u32, u32)> {
    let (first, last) = word.split_once('-')?;
    let first = season(first).filter(|_| first.starts_with(['S', 's']))?;
    let last = season(last).or_else(|| digits_only(last))?;
    (first < last).then_some((first, last))
}

/// `Batch` or `Complete`.
pub(crate) fn is_batch(word: &str) -> bool {
    word.eq_ignore_ascii_case("batch") || word.eq_ignore_ascii_case("complete")
}

/// A volume glued to its number: `Vol.1`, `Vol1`, `Vol.01`.
pub(crate) fn volume(word: &str) -> Option<u32> {
    let lower = word.to_ascii_lowercase();
    let rest = lower.strip_prefix("vol")?;
    volume_number(rest.strip_prefix('.').unwrap_or(rest))
}

/// A volume number, or the first of a volume range: `1`, `01`, `1-2`, `2~3`.
pub(crate) fn volume_number(word: &str) -> Option<u32> {
    digits_only(word).or_else(|| episode_range(word).map(|(first, _)| first))
}

/// A bare `Vol`, `Vol.` or `Volume`, whose number is the next word.
pub(crate) fn is_volume_keyword(word: &str) -> bool {
    ["vol", "vol.", "volume"].contains(&word.to_ascii_lowercase().as_str())
}

/// `第01話`.
pub(crate) fn japanese_episode(word: &str) -> Option<u32> {
    digits_only(word.strip_prefix('第')?.strip_suffix('話')?)
}

/// A kind word with an optional glued number: `NCOP`, `NCOP2`, `SP01`, `OVA`.
///
/// The two-letter words (`OP`, `ED`, `SP`, `PV`, `CM`) count on their own only
/// in capitals, so `Ed` in a title stays a word.
pub(crate) fn kind_word(word: &str) -> Option<(EpisodeKind, Option<Numbered>)> {
    let lower = word.to_ascii_lowercase();
    KINDS.iter().find_map(|&(keyword, kind)| {
        let rest = lower.strip_prefix(keyword)?;
        if rest.is_empty() {
            let shouted = keyword.len() > 2 || word.chars().all(|c| c.is_ascii_uppercase());
            return shouted.then_some((kind, None));
        }
        number(rest).map(|numbered| (kind, Some(numbered)))
    })
}
