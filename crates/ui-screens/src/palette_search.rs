//! The local-first row logic behind the palette's search modes (#63).
//!
//! Rows are plain data so ordering, de-duplication and the offline rule can
//! be tested without a window or a network. The Local platform stores no
//! titles today, so an untitled local entry cannot be ruled out by text and
//! is shown above the remote results; titled entries are fuzzy-matched with
//! `nucleo-matcher`.

use std::collections::HashSet;

use domain::{ExternalId, MediaKind};
use metadata::{MediaSummary, MediaTitle};
use nucleo_matcher::{
    Config, Matcher, Utf32Str,
    pattern::{AtomKind, CaseMatching, Normalization, Pattern},
};

/// How many remote rows a search mode shows.
pub(crate) const MAX_RESULTS: usize = 10;

/// How long the palette waits after the last query change before asking
/// AniList (#33: 350ms).
pub(crate) const DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(350);

/// At most this many recent searches are kept for the empty-query mode.
const HISTORY_LIMIT: usize = 8;

/// A local list entry's row, keyed by the AniList id it is linked to.
pub(crate) fn local_summary(kind: MediaKind, ids: &[ExternalId]) -> MediaSummary {
    let anilist_id = ids
        .iter()
        .find_map(|id| match id {
            ExternalId::AniList(id) => Some(*id),
            _ => None,
        })
        .unwrap_or_default();
    let mal_id = ids.iter().find_map(|id| match id {
        ExternalId::Mal(id) => Some(*id),
        _ => None,
    });
    MediaSummary {
        anilist_id,
        mal_id,
        kind: Some(kind),
        title: MediaTitle::default(),
        poster: None,
        banner: None,
        format: None,
        status: None,
        season: None,
        season_year: None,
        episodes: None,
        chapters: None,
        mean_score: None,
        genres: Vec::new(),
        is_adult: false,
    }
}

/// The text a row can be fuzzy-matched against, if it has any.
fn match_text(media: &MediaSummary) -> Option<&str> {
    media
        .title
        .romaji
        .as_deref()
        .or(media.title.english.as_deref())
        .or(media.title.native.as_deref())
}

/// What the palette labels a media row with.
pub(crate) fn display_label(media: &MediaSummary) -> String {
    match_text(media).map_or_else(|| format!("Media #{}", media.anilist_id), str::to_owned)
}

/// The local entries matching `query`, in their stored order. An entry with
/// no title is kept: the Local platform has no title to compare with yet.
pub(crate) fn local_matches(query: &str, local: &[MediaSummary]) -> Vec<MediaSummary> {
    let mut matcher = Matcher::new(Config::DEFAULT);
    let pattern = Pattern::new(
        query,
        CaseMatching::Smart,
        Normalization::Smart,
        AtomKind::Fuzzy,
    );
    local
        .iter()
        .filter(|media| match match_text(media) {
            None => true,
            Some(text) => {
                let mut buffer = Vec::new();
                pattern
                    .score(Utf32Str::new(text, &mut buffer), &mut matcher)
                    .is_some()
            }
        })
        .cloned()
        .collect()
}

/// The identity used for de-duplication: AniList id first, then the
/// MyAnimeList id for media AniList does not link to (#33). Rows with neither
/// are local entries that have no cross-reference yet.
fn key(media: &MediaSummary, index: usize) -> String {
    if media.anilist_id != 0 {
        format!("anilist:{}", media.anilist_id)
    } else if let Some(mal) = media.mal_id {
        format!("mal:{mal}")
    } else {
        format!("local:{index}")
    }
}

/// Local matches first, then remote results that are not already on screen,
/// de-duplicated by external id (locals win). Both together are capped at
/// [`MAX_RESULTS`]. `None` remote rows mean the search is offline or has not
/// answered yet.
pub(crate) fn merge(
    local: Vec<MediaSummary>,
    remote: Option<Vec<MediaSummary>>,
) -> Vec<MediaSummary> {
    let mut seen: HashSet<String> = HashSet::new();
    for (index, media) in local.iter().enumerate() {
        seen.insert(key(media, index));
    }
    let remote: Vec<MediaSummary> = remote
        .unwrap_or_default()
        .into_iter()
        .filter(|media| seen.insert(key(media, 0)))
        .collect();
    local.into_iter().chain(remote).take(MAX_RESULTS).collect()
}

/// Whether a search mode should ask AniList for `text`: at least two
/// characters, so a single keystroke does not spend a request (#33).
pub(crate) fn needs_remote(text: &str) -> bool {
    text.chars().count() >= 2
}

/// Recent searches, most recent first, without duplicates or more than
/// [`HISTORY_LIMIT`] entries.
pub(crate) fn remember(history: &[String], text: &str) -> Vec<String> {
    if text.is_empty() {
        return history.to_vec();
    }
    std::iter::once(text.to_owned())
        .chain(history.iter().filter(|old| old.as_str() != text).cloned())
        .take(HISTORY_LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "tests index known rows to assert their order"
    )]

    use domain::{ExternalId, MediaKind};
    use metadata::MediaTitle;

    use super::{display_label, local_matches, local_summary, merge, needs_remote, remember};

    fn titled(id: u32, title: &str) -> metadata::MediaSummary {
        metadata::MediaSummary {
            anilist_id: id,
            title: MediaTitle {
                romaji: Some(title.to_owned()),
                ..MediaTitle::default()
            },
            ..local_summary(MediaKind::Anime, &[ExternalId::AniList(id)])
        }
    }

    #[test]
    fn a_local_entry_is_keyed_by_its_anilist_id() {
        let row = local_summary(
            MediaKind::Anime,
            &[ExternalId::Mal(7), ExternalId::AniList(42)],
        );
        assert_eq!(row.anilist_id, 42);
        assert_eq!(row.mal_id, Some(7));
        assert_eq!(row.kind, Some(MediaKind::Anime));
        assert_eq!(display_label(&row), "Media #42");
    }

    #[test]
    fn titled_local_entries_are_fuzzy_matched() {
        let local = [titled(1, "Sousou no Frieren"), titled(2, "Jujutsu Kaisen")];
        let found = local_matches("frie", &local);
        assert_eq!(found.iter().map(|m| m.anilist_id).collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn untitled_local_entries_are_not_ruled_out_by_text() {
        let local = [local_summary(MediaKind::Anime, &[ExternalId::AniList(9)])];
        assert_eq!(local_matches("anything", &local).len(), 1);
    }

    #[test]
    fn local_rows_come_above_remote_ones_and_win_on_anilist_id() {
        let local = [titled(1, "Local one"), titled(2, "Local two")];
        let remote = [titled(1, "Remote duplicate"), titled(3, "Remote three")];
        let rows = merge(local.to_vec(), Some(remote.to_vec()));
        assert_eq!(
            rows.iter().map(|m| m.anilist_id).collect::<Vec<_>>(),
            [1, 2, 3]
        );
        assert_eq!(display_label(&rows[0]), "Local one");
    }

    #[test]
    fn remote_results_are_capped_at_ten() {
        let remote: Vec<_> = (0..12).map(|id| titled(id, "Remote")).collect();
        assert_eq!(merge(Vec::new(), Some(remote)).len(), 10);
    }

    #[test]
    fn local_entries_without_an_anilist_id_stay_distinct() {
        let local = [titled(0, "First"), titled(0, "Second")];
        let mut first = local[0].clone();
        first.anilist_id = 0;
        first.mal_id = Some(11);
        let mut second = local[1].clone();
        second.anilist_id = 0;
        second.mal_id = Some(22);
        let rows = merge(vec![first, second], None);
        assert_eq!(rows.len(), 2, "distinct MAL ids are distinct rows");
    }

    #[test]
    fn an_offline_search_keeps_the_local_rows_only() {
        let local = [titled(1, "Local one")];
        let rows = merge(local.to_vec(), None);
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].anilist_id, 1);
    }

    #[test]
    fn a_single_character_does_not_trigger_a_request() {
        assert!(!needs_remote("a"));
        assert!(needs_remote("ab"));
    }

    #[test]
    fn recent_searches_are_most_recent_first_and_capped() {
        let history = remember(&[], "frieren");
        assert_eq!(history, ["frieren"]);
        assert_eq!(remember(&history, "frieren"), ["frieren"]);
        let filled: Vec<String> = (0..9).map(|n| format!("q{n}")).collect();
        let trimmed = remember(&filled, "newest");
        assert_eq!(trimmed.len(), 8);
        assert_eq!(trimmed[0], "newest");
    }
}
