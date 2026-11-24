//! Turns an anime release filename or folder name into a [`Release`].
//!
//! Pure functions, no I/O. See ARCHITECTURE §2.
//!
//! A name reads left to right in three zones: the title, then (after an
//! episode marker such as `- 01` or `S01E05`) the episode title, then the
//! technical tail that starts at the first tag such as `1080p` or `x265`.
//! Brackets anywhere can carry the group, resolution, checksum or year.

mod tag;
mod token;

use tag::{EpisodePrefix, Numbered};
use token::{Dots, Token};

/// What a release name says about its file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct Release {
    /// The series title, as written in the name.
    pub title: Option<String>,
    /// The release group, from `[Group]` or a scene `-GROUP` suffix.
    pub group: Option<String>,
    /// The season, from `S02`, `Season 2` or `2nd Season`. `S00` is specials.
    pub season: Option<u32>,
    /// The number within its [`kind`](Self::kind): episode 5, `SP01`, `NCOP2`.
    pub episode: Option<u32>,
    /// The release version, from `01v2`.
    pub version: Option<u32>,
    /// The episode's own title, after the episode number.
    pub episode_title: Option<String>,
    /// The vertical resolution in lines: 1080 for `1080p` or `1920x1080`.
    pub resolution: Option<u32>,
    /// The lowercase video extension, absent for folder names.
    pub extension: Option<String>,
    /// The CRC32 checksum from an eight-digit hexadecimal bracket.
    pub checksum: Option<u32>,
    /// The year, from `(2016)` or a scene `.2016.`.
    pub year: Option<u32>,
    /// What kind of video the file is.
    pub kind: EpisodeKind,
}

/// What kind of video a release is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EpisodeKind {
    /// A regular episode, or a name without any kind marker.
    #[default]
    Episode,
    /// A special: `SP`, `Special`, `OVA`, `OAD`, `ONA` or season `S00`.
    Special,
    /// A movie, marked `Movie`, `The Movie` or a leading `Gekijouban`.
    Movie,
    /// A creditless opening: `NCOP`, `NC OP`, `OP` or `Opening`.
    Ncop,
    /// A creditless ending: `NCED`, `NC ED`, `ED` or `Ending`.
    Nced,
    /// Another extra: `PV`, `CM`, `Menu` or `Preview`.
    Extra,
}

/// Parses a release filename or folder name.
///
/// Never fails: a field the name doesn't state stays `None`.
#[must_use]
pub fn parse(name: &str) -> Release {
    let (stem, extension) = tag::split_extension(name.trim());
    let text = stem.replace('_', " ");
    let dots = if !text.contains(' ') && text.contains('.') {
        Dots::Separate
    } else {
        Dots::Keep
    };
    let tokens = token::tokenize(&text, dots);
    let mut walk = Walk {
        release: Release {
            extension,
            ..Release::default()
        },
        zone: Zone::Title,
        title: Vec::new(),
        episode_title: Vec::new(),
    };
    walk.run(&tokens);
    walk.finish(&tokens)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Zone {
    Title,
    EpisodeTitle,
    Tail,
}

#[derive(Debug, PartialEq, Eq)]
enum Part {
    Word(String),
    Dash,
    Slash,
}

struct Walk {
    release: Release,
    zone: Zone,
    title: Vec<Part>,
    episode_title: Vec<String>,
}

impl Walk {
    fn run(&mut self, tokens: &[Token]) {
        let mut i = self.take_group(tokens);
        while let Some(token) = tokens.get(i) {
            let next = tokens.get(i + 1);
            let later = tokens.get(i + 1..).unwrap_or_default();
            let consumed = match (self.zone, token) {
                (Zone::Title, Token::Word(word)) => self.title_word(word, next),
                (Zone::Title, Token::Dash) => {
                    self.title.push(Part::Dash);
                    0
                }
                (Zone::Title, Token::Slash) => {
                    self.title.push(Part::Slash);
                    0
                }
                (Zone::EpisodeTitle, Token::Word(word)) => {
                    self.episode_title_word(word, next);
                    0
                }
                (Zone::EpisodeTitle, Token::Dash) => {
                    if !self.episode_title.is_empty() {
                        self.episode_title.push("-".to_owned());
                    }
                    0
                }
                (Zone::EpisodeTitle, Token::Slash) => {
                    self.zone = Zone::Tail;
                    0
                }
                (_, Token::Bracket { open, content }) => {
                    self.bracket(*open, content.trim(), later);
                    0
                }
                (Zone::Tail, _) => 0,
            };
            i += 1 + consumed;
        }
    }

    /// Takes a leading `[Group]` and the dashes after it. Returns the next index.
    fn take_group(&mut self, tokens: &[Token]) -> usize {
        let mut i = 0;
        if let Some(Token::Bracket {
            open: '[' | '【',
            content,
        }) = tokens.first()
            && !content.trim().is_empty()
            && !is_metadata(content)
        {
            self.release.group = Some(content.trim().to_owned());
            i = 1;
        }
        while tokens.get(i) == Some(&Token::Dash) {
            i += 1;
        }
        i
    }

    /// Handles a word in the title zone. Returns how many following tokens it used.
    fn title_word(&mut self, word: &str, next: Option<&Token>) -> usize {
        if let Some(found) = tag::season_episode(word) {
            self.release.season = Some(found.season);
            if let Some((episode, version)) = found.episode {
                self.start_episode(episode, version);
                if found.season == 0 {
                    self.release.kind = EpisodeKind::Special;
                }
            }
            if let Some((kind, numbered)) = found.kind {
                self.start_kind(kind, numbered);
            }
            if !found.rest.is_empty() {
                self.episode_title.push(found.rest.to_owned());
            }
            return 0;
        }
        if let Some(season) = tag::season(word) {
            self.release.season = Some(season);
            return 0;
        }
        if word.eq_ignore_ascii_case("season")
            && let Some((season, None)) = next_number(next)
        {
            self.release.season = Some(season);
            return 1;
        }
        if let Some(season) = tag::ordinal(word)
            && matches!(next, Some(Token::Word(w)) if w.eq_ignore_ascii_case("season"))
        {
            self.release.season = Some(season);
            return 1;
        }
        match tag::episode_prefix(word) {
            Some(EpisodePrefix::Numbered((episode, version))) => {
                self.start_episode(episode, version);
                return 0;
            }
            Some(EpisodePrefix::NumberFollows) => {
                if let Some((episode, version)) = next_number(next) {
                    self.start_episode(episode, version);
                    return 1;
                }
            }
            None => {}
        }
        if let Some(episode) = tag::japanese_episode(word) {
            self.start_episode(episode, None);
            return 0;
        }
        if word.eq_ignore_ascii_case("nc")
            && let Some(Token::Word(next_word)) = next
            && let Some((kind @ (EpisodeKind::Ncop | EpisodeKind::Nced), numbered)) =
                tag::kind_word(next_word)
        {
            self.start_kind(kind, numbered);
            return 1;
        }
        let segment_start = matches!(self.title.last(), None | Some(Part::Slash));
        if word.eq_ignore_ascii_case("gekijouban") && segment_start {
            self.release.kind = EpisodeKind::Movie;
            return 0;
        }
        if let Some((kind, numbered)) = tag::kind_word(word) {
            if numbered.is_some() {
                self.start_kind(kind, numbered);
                return 0;
            }
            if closes_kind_word(next) && self.kind_word_fits(word, next) {
                if kind == EpisodeKind::Movie && self.last_title_word_is("the") {
                    self.title.pop();
                }
                let numbered = next_number(next);
                self.start_kind(kind, numbered);
                return usize::from(numbered.is_some());
            }
        }
        if tag::ends_title(word) {
            self.zone = Zone::Tail;
            return 0;
        }
        if tag::number(word).is_some() && self.has_title() {
            if self.title.last() == Some(&Part::Dash)
                && let Some((episode, version)) = tag::number(word)
            {
                self.start_episode(episode, version);
                return 0;
            }
            if let Some(year) = tag::year(word) {
                self.release.year = Some(year);
                return 0;
            }
        }
        self.title.push(Part::Word(word.to_owned()));
        0
    }

    /// Whether a bare dictionary kind word such as `Ending` marks the kind here,
    /// rather than being part of the title as in `Happy Ending - 01`. Acronyms
    /// such as `OVA` always do.
    fn kind_word_fits(&self, word: &str, next: Option<&Token>) -> bool {
        !tag::is_dictionary_kind(word)
            || !self.has_title()
            || self.title.last() == Some(&Part::Dash)
            || self.last_title_word_is("the")
            || matches!(next, Some(Token::Word(w)) if tag::number(w).is_some() || tag::is_tech(w))
    }

    fn last_title_word_is(&self, word: &str) -> bool {
        matches!(self.title.last(), Some(Part::Word(w)) if w.eq_ignore_ascii_case(word))
    }

    fn episode_title_word(&mut self, word: &str, next: Option<&Token>) {
        if tag::is_tech(word) {
            self.zone = Zone::Tail;
            return;
        }
        // A kind word closing the episode title: `Title - 05 Preview [1080p]`,
        // `Title - 24 Next Ep PV`. A dictionary word counts only on its own.
        if self.release.kind == EpisodeKind::Episode
            && (self.episode_title.is_empty() || !tag::is_dictionary_kind(word))
            && let Some((kind, None)) = tag::kind_word(word)
            && closes_kind_word(next)
            && !matches!(next, Some(Token::Dash))
        {
            self.release.kind = kind;
            return;
        }
        if self.release.episode.is_none()
            && self.episode_title.is_empty()
            && let Some((episode, version)) = tag::number(word)
        {
            self.release.episode = Some(episode);
            self.release.version = version;
            return;
        }
        self.episode_title.push(word.to_owned());
    }

    /// Reads a bracket's contents. `later` is every token after it.
    fn bracket(&mut self, open: char, content: &str, later: &[Token]) {
        if let Some(checksum) = tag::checksum(content) {
            self.release.checksum = Some(checksum);
            return;
        }
        let words: Vec<&str> = content
            .split([' ', ',', ';'])
            .filter(|w| !w.is_empty())
            .collect();
        // A kind in its own bracket: `Title (OVA) - 01`, `Title - 01 (OVA)`.
        if let [word] = words.as_slice()
            && self.release.kind == EpisodeKind::Episode
            && let Some((kind, numbered)) = tag::kind_word(word)
        {
            self.release.kind = kind;
            if self.release.episode.is_none()
                && let Some((episode, version)) = numbered
            {
                self.release.episode = Some(episode);
                self.release.version = version;
            }
            if self.zone == Zone::EpisodeTitle {
                self.zone = Zone::Tail;
            }
            return;
        }
        match self.zone {
            Zone::Title => {
                if let Some(year) = tag::year(content) {
                    self.release.year = Some(year);
                } else if let [season_word, number] = words.as_slice()
                    && season_word.eq_ignore_ascii_case("season")
                    && let Some((season, None)) = tag::number(number)
                {
                    self.release.season = Some(season);
                } else if self.release.episode.is_none()
                    && let Some((episode, version)) = tag::number(content)
                {
                    self.start_episode(episode, version);
                } else if open == '('
                    && self.has_title()
                    && !words.iter().any(|w| tag::is_tech(w))
                    && later.iter().any(|t| matches!(t, Token::Word(_)))
                {
                    // A parenthesis inside the title, as in `Nekomonogatari (Kuro) - 01`.
                    self.title.push(Part::Word(format!("({content})")));
                }
            }
            Zone::EpisodeTitle => {
                if open == '「' && self.episode_title.is_empty() {
                    self.episode_title.push(content.to_owned());
                }
                self.zone = Zone::Tail;
            }
            Zone::Tail => {}
        }
    }

    fn start_episode(&mut self, episode: u32, version: Option<u32>) {
        self.release.episode = Some(episode);
        self.release.version = version;
        self.zone = Zone::EpisodeTitle;
    }

    fn start_kind(&mut self, kind: EpisodeKind, numbered: Option<Numbered>) {
        self.release.kind = kind;
        self.zone = Zone::EpisodeTitle;
        if let Some((episode, version)) = numbered {
            self.start_episode(episode, version);
        }
    }

    fn has_title(&self) -> bool {
        self.title.iter().any(|part| matches!(part, Part::Word(_)))
    }

    fn finish(mut self, tokens: &[Token]) -> Release {
        // A zero-padded number closing the title is the episode: `Bakemonogatari_01_(…)`.
        if self.release.episode.is_none()
            && let Some(Part::Word(last)) = self.title.last()
            && last.starts_with('0')
            && (2..=3).contains(&last.chars().take_while(char::is_ascii_digit).count())
            && let Some((episode, version)) = tag::number(last)
        {
            self.release.episode = Some(episode);
            self.release.version = version;
            self.title.pop();
        }
        self.release.title = assemble_title(&self.title);
        let episode_title = self.episode_title.join(" ");
        let episode_title = episode_title.trim_end_matches([' ', '-']);
        self.release.episode_title = (!episode_title.is_empty()).then(|| episode_title.to_owned());
        self.release.resolution = tokens.iter().find_map(|token| match token {
            Token::Word(word) => tag::resolution(word),
            Token::Bracket { content, .. } => {
                content.split([' ', ',', '.']).find_map(tag::resolution)
            }
            Token::Dash | Token::Slash => None,
        });
        if self.release.group.is_none() && self.zone == Zone::Tail {
            let last_word = tokens.iter().rev().find_map(|token| match token {
                Token::Word(word) => Some(word),
                _ => None,
            });
            self.release.group = last_word.and_then(|word| scene_group(word));
        }
        self.release
    }
}

/// The group in a scene suffix such as `x264-GROUP` or `H.264-VARYG`.
fn scene_group(word: &str) -> Option<String> {
    if tag::is_tech(word) {
        return None;
    }
    word.match_indices('-').find_map(|(at, _)| {
        let (tag, group) = word.split_at_checked(at)?;
        let group = group.strip_prefix('-')?;
        // In a scene name `H.264-VARYG` splits into `H` and `264-VARYG`.
        let tag_ok = tag::is_tech(tag) || tag == "264" || tag == "265";
        (tag_ok && group.chars().any(char::is_alphabetic)).then(|| group.to_owned())
    })
}

/// Joins the title words. Between alternative titles split by `/`, the first
/// one written in Latin letters wins.
fn assemble_title(parts: &[Part]) -> Option<String> {
    let latin =
        |part: &Part| matches!(part, Part::Word(w) if w.chars().any(|c| c.is_ascii_alphabetic()));
    let mut segments = parts.split(|part| *part == Part::Slash);
    let mut chosen = segments
        .clone()
        .find(|segment| segment.iter().any(latin))
        .or_else(|| segments.find(|segment| !segment.is_empty()))?;
    while let [Part::Dash, rest @ ..] | [rest @ .., Part::Dash] = chosen {
        chosen = rest;
    }
    let words: Vec<&str> = chosen
        .iter()
        .map(|part| match part {
            Part::Word(word) => word.as_str(),
            Part::Dash => "-",
            Part::Slash => "/",
        })
        .collect();
    let title = words.join(" ");
    let title = title.trim_end_matches([':', ',']);
    (!title.is_empty()).then(|| title.to_owned())
}

/// Bracket contents that can't be a group name.
fn is_metadata(content: &str) -> bool {
    let content = content.trim();
    tag::checksum(content).is_some()
        || tag::number(content).is_some()
        || content.split(' ').any(|w| tag::resolution(w).is_some())
}

fn next_number(next: Option<&Token>) -> Option<Numbered> {
    match next {
        Some(Token::Word(word)) => tag::number(word),
        _ => None,
    }
}

/// Whether a bare kind word followed by `next` marks the kind, rather than
/// being part of the title (`Title OVA - 01`, but `Special A`).
fn closes_kind_word(next: Option<&Token>) -> bool {
    match next {
        None | Some(Token::Dash | Token::Slash | Token::Bracket { .. }) => true,
        Some(Token::Word(word)) => tag::number(word).is_some() || tag::is_tech(word),
    }
}
