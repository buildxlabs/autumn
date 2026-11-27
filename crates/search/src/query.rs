//! Query construction and the request budget (#181 Q1–Q4).

use std::collections::HashSet;

use crate::result::Indexer;

/// At most three ordered title variants are searched per indexer.
pub const MAX_TITLES: usize = 3;
/// Hard cap on requests to one indexer for a single search.
pub const MAX_REQUESTS_PER_INDEXER: usize = 8;

/// The media and episode a search is for.
///
/// Titles are supplied in priority order: romaji, English, native, then
/// `AniList` / anime-offline-database / `AniZip` synonyms. `AniDB` ids are optional:
/// #215 supplies the episode id, and `AnimeTosho` uses the anime id when present.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct SearchRequest {
    /// Ordered known titles; season markers are stripped during construction.
    pub titles: Vec<String>,
    /// This media's season `M`, from `AniZip`, the title, or 1.
    pub season: u32,
    /// The requested episode `N` within this media.
    pub episode: u32,
    /// `AniZip` `absolute_number` when it differs from [`Self::episode`].
    pub absolute_episode: Option<u32>,
    /// `AniDB` anime id (`aid`), when known.
    pub anidb_aid: Option<u32>,
    /// `AniDB` episode id (`eid`), when known; never required.
    pub anidb_eid: Option<u32>,
}

impl SearchRequest {
    /// A request with no absolute number and no `AniDB` ids.
    #[must_use]
    pub fn new(titles: Vec<String>, season: u32, episode: u32) -> Self {
        Self {
            titles,
            season,
            episode,
            absolute_episode: None,
            anidb_aid: None,
            anidb_eid: None,
        }
    }

    /// Add an absolute episode number when it differs from the seasonal one.
    #[must_use]
    pub fn with_absolute_episode(mut self, absolute: u32) -> Self {
        self.absolute_episode = Some(absolute);
        self
    }

    /// Add the `AniDB` anime id and, optionally, the `AniDB` episode id.
    #[must_use]
    pub fn with_anidb_ids(mut self, aid: u32, eid: Option<u32>) -> Self {
        self.anidb_aid = Some(aid);
        self.anidb_eid = eid;
        self
    }
}

/// One query sent to one indexer.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum Query {
    /// A keyword query on `Nyaa`; either numbered or the keyword fallback.
    Nyaa {
        /// The cleaned keyword, zero-padded when numbered.
        keyword: String,
    },
    /// A query on `AnimeTosho`'s JSON feed.
    AnimeTosho {
        /// Free-text `q=` value, absent for id-only queries.
        keyword: Option<String>,
        /// `AniDB` anime id (`aid=`).
        aid: Option<u32>,
        /// `AniDB` episode id (`eid=`).
        eid: Option<u32>,
        /// `AnimeTosho` `season=` parameter for numbered queries.
        season: Option<u32>,
        /// `AnimeTosho` `ep=` parameter for numbered queries.
        episode: Option<u32>,
    },
}

impl Query {
    /// Build a seasonal keyword query with `season=` and `ep=` parameters.
    #[must_use]
    pub fn anime_tosho_keyword(keyword: &str, season: u32, episode: u32) -> Self {
        Self::AnimeTosho {
            keyword: Some(keyword.to_owned()),
            aid: None,
            eid: None,
            season: Some(season),
            episode: Some(episode),
        }
    }

    /// Build an absolute-number keyword query on `AnimeTosho`.
    #[must_use]
    pub fn anime_tosho_absolute(keyword: &str, absolute: u32) -> Self {
        Self::AnimeTosho {
            keyword: Some(keyword.to_owned()),
            aid: None,
            eid: None,
            season: None,
            episode: Some(absolute),
        }
    }

    /// A keyword query looking for a batch on `AnimeTosho`.
    #[must_use]
    pub fn anime_tosho_batch(keyword: &str) -> Self {
        Self::AnimeTosho {
            keyword: Some(format!("{keyword} batch")),
            aid: None,
            eid: None,
            season: None,
            episode: None,
        }
    }

    /// Query `AnimeTosho` by `AniDB` anime id alone.
    #[must_use]
    pub fn anime_tosho_aid(aid: u32) -> Self {
        Self::AnimeTosho {
            keyword: None,
            aid: Some(aid),
            eid: None,
            season: None,
            episode: None,
        }
    }

    /// Query `AnimeTosho` by both `AniDB` ids.
    #[must_use]
    pub fn anime_tosho_eid(aid: u32, eid: u32) -> Self {
        Self::AnimeTosho {
            keyword: None,
            aid: Some(aid),
            eid: Some(eid),
            season: None,
            episode: None,
        }
    }

    /// The keyword-only last resort on `AnimeTosho`.
    #[must_use]
    pub fn anime_tosho_fallback(keyword: &str) -> Self {
        Self::AnimeTosho {
            keyword: Some(keyword.to_owned()),
            aid: None,
            eid: None,
            season: None,
            episode: None,
        }
    }

    /// The indexer this query targets.
    #[must_use]
    pub const fn indexer(&self) -> Indexer {
        match self {
            Self::Nyaa { .. } => Indexer::Nyaa,
            Self::AnimeTosho { .. } => Indexer::AnimeTosho,
        }
    }
}

/// The queries to run, split into numbered queries and last-resort fallbacks.
///
/// Fallbacks are only issued when the numbered queries for that indexer found
/// nothing. Both groups are already inside the per-indexer budget.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct QueryPlan {
    /// Numbered or id-keyed queries, in priority order.
    pub numbered: Vec<Query>,
    /// Keyword-only queries, attempted only when `numbered` found nothing.
    pub fallback: Vec<Query>,
}

/// Clean a known title for keyword search: punctuation and season markers go,
/// case is folded, and whitespace is collapsed. `None` when nothing remains.
#[must_use]
pub fn clean_title(title: &str) -> Option<String> {
    let words = strip_season_markers(title);
    let plain: String = words
        .chars()
        .filter(|character| character.is_alphanumeric() || character.is_whitespace())
        .collect();
    let cleaned = plain
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase();
    (!cleaned.is_empty()).then_some(cleaned)
}

/// Construct a budgeted query plan from a media/episode request.
#[must_use]
pub fn plan(request: &SearchRequest) -> QueryPlan {
    let titles = ordered_titles(request);
    let absolute = request
        .absolute_episode
        .filter(|absolute| *absolute != request.episode);

    let mut nyaa = IndexerQueries::default();
    for title in &titles {
        nyaa.numbered.push(Query::Nyaa {
            keyword: numbered(title, request.episode),
        });
        if let Some(absolute) = absolute {
            nyaa.numbered.push(Query::Nyaa {
                keyword: numbered(title, absolute),
            });
        }
    }
    if let Some(romaji) = titles.first() {
        nyaa.numbered.push(Query::Nyaa {
            keyword: format!("{romaji} batch"),
        });
        nyaa.fallback.push(Query::Nyaa {
            keyword: romaji.clone(),
        });
    }

    let mut tosho = IndexerQueries::default();
    if let Some(aid) = request.anidb_aid {
        tosho.numbered.push(Query::anime_tosho_aid(aid));
        if let Some(eid) = request.anidb_eid {
            tosho.numbered.push(Query::anime_tosho_eid(aid, eid));
        }
    } else {
        for title in &titles {
            tosho.numbered.push(Query::anime_tosho_keyword(
                title,
                request.season,
                request.episode,
            ));
            if let Some(absolute) = absolute {
                tosho
                    .numbered
                    .push(Query::anime_tosho_absolute(title, absolute));
            }
        }
        if let Some(eid) = request.anidb_eid {
            tosho.numbered.push(Query::AnimeTosho {
                keyword: None,
                aid: None,
                eid: Some(eid),
                season: None,
                episode: None,
            });
        }
        if let Some(romaji) = titles.first() {
            tosho.numbered.push(Query::anime_tosho_batch(romaji));
        }
    }
    if let Some(romaji) = titles.first() {
        tosho.fallback.push(Query::anime_tosho_fallback(romaji));
    }

    let nyaa = nyaa.budget();
    let tosho = tosho.budget();
    QueryPlan {
        numbered: [nyaa.numbered, tosho.numbered].concat(),
        // AnimeTosho's `q=` fallback is the more precise one on a bad title.
        fallback: [tosho.fallback, nyaa.fallback].concat(),
    }
}

#[derive(Default)]
struct IndexerQueries {
    numbered: Vec<Query>,
    fallback: Vec<Query>,
}

impl IndexerQueries {
    fn budget(mut self) -> Self {
        self.numbered = dedup(self.numbered);
        self.numbered.truncate(MAX_REQUESTS_PER_INDEXER - 1);
        self.fallback = dedup(self.fallback);
        self.fallback
            .truncate(MAX_REQUESTS_PER_INDEXER.saturating_sub(self.numbered.len()));
        self
    }
}

fn dedup(queries: Vec<Query>) -> Vec<Query> {
    let mut seen = HashSet::new();
    queries
        .into_iter()
        .filter(|query| seen.insert(query.clone()))
        .collect()
}

fn ordered_titles(request: &SearchRequest) -> Vec<String> {
    let mut seen = HashSet::new();
    request
        .titles
        .iter()
        .filter_map(|title| clean_title(title))
        .filter(|title| seen.insert(title.clone()))
        .take(MAX_TITLES)
        .collect()
}

fn numbered(title: &str, episode: u32) -> String {
    if episode < 100 {
        format!("{title} {episode:02}")
    } else {
        format!("{title} {episode}")
    }
}

fn strip_season_markers(title: &str) -> String {
    let words: Vec<&str> = title.split_whitespace().collect();
    let key = |word: &str| -> String {
        word.chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect()
    };
    let mut kept = Vec::new();
    let mut index = 0;
    while let Some(word) = words.get(index) {
        let lower = key(word);
        if is_season_code(&lower) {
            index += 1;
        } else if lower == "season" {
            index += 1;
            if words
                .get(index)
                .is_some_and(|next| is_number_word(&key(next)))
            {
                index += 1;
            }
        } else if is_ordinal(&lower)
            && words
                .get(index + 1)
                .is_some_and(|next| key(next) == "season")
        {
            index += 2;
        } else {
            kept.push(*word);
            index += 1;
        }
    }
    kept.join(" ")
}

fn is_season_code(word: &str) -> bool {
    let digits = word.strip_prefix('s').unwrap_or_default();
    (2..=4).contains(&word.len())
        && !digits.is_empty()
        && digits.chars().all(|character| character.is_ascii_digit())
}

fn is_number_word(word: &str) -> bool {
    word.chars().all(|character| character.is_ascii_digit()) || is_ordinal(word)
}

fn is_ordinal(word: &str) -> bool {
    let digits = word
        .strip_suffix("st")
        .or_else(|| word.strip_suffix("nd"))
        .or_else(|| word.strip_suffix("rd"))
        .or_else(|| word.strip_suffix("th"))
        .unwrap_or_default();
    !digits.is_empty() && digits.chars().all(|character| character.is_ascii_digit())
}
