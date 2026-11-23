//! My Lists state: tabs, inline search, sort and quick edits (ticket #60).
//!
//! The session is independent of GPUI and of any platform so grouping,
//! filtering and the write path can be tested at one seam. The active
//! platform is read by a [`ListSource`] supplied by the composition root;
//! today that source is the account-free Local platform in SQLite, which
//! makes no network requests.

use std::future::Future;

use domain::{ListEntry, ListStatus, MediaId, Score};

/// The order status tabs appear in, matching the tracking platform.
/// The order status tabs appear in, matching the tracking platform. Exposed
/// so screens can offer the same cycle without a second copy.
pub const STATUS_ORDER: [ListStatus; 6] = [
    ListStatus::Current,
    ListStatus::Repeating,
    ListStatus::Planning,
    ListStatus::Completed,
    ListStatus::Paused,
    ListStatus::Dropped,
];

/// One tab in My Lists: a status or a custom list.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum ListTab {
    /// Every entry with this status that is not hidden from status lists.
    Status(ListStatus),
    /// Every entry in the custom list of this name.
    Custom(String),
}

impl Default for ListTab {
    fn default() -> Self {
        Self::Status(ListStatus::Current)
    }
}

/// The order cards are shown in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ListSort {
    /// Most recently changed first.
    #[default]
    Recent,
    /// Title, case-insensitively.
    Title,
    /// Highest progress first.
    Progress,
    /// Highest score first.
    Score,
}

/// The fields a quick editor can change; `None` leaves a field alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct QuickEdit {
    /// A new status, if chosen.
    pub status: Option<ListStatus>,
    /// A new progress value, if entered.
    pub progress: Option<u32>,
    /// A new score, if entered.
    pub score: Option<Score>,
}

impl QuickEdit {
    /// An edit that changes nothing.
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the new status.
    #[must_use]
    pub fn status(mut self, status: ListStatus) -> Self {
        self.status = Some(status);
        self
    }

    /// Sets the new progress.
    #[must_use]
    pub fn progress(mut self, progress: u32) -> Self {
        self.progress = Some(progress);
        self
    }

    /// Sets the new score.
    #[must_use]
    pub fn score(mut self, score: Score) -> Self {
        self.score = Some(score);
        self
    }
}

/// A list entry with the catalog text needed to render its card.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListCard {
    /// The tracking record, with status, progress, score and custom lists.
    pub entry: ListEntry,
    /// The title cached from the catalog, or a fallback for uncached media.
    pub title: String,
    /// The poster URL cached from the catalog, when available.
    pub poster: Option<String>,
}

/// Why reading or writing the active platform failed.
#[derive(Debug, thiserror::Error)]
#[error("list source failed: {message}")]
pub struct ListSourceError {
    message: String,
}

impl ListSourceError {
    /// A source error with a message safe to show in the UI.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// Reads list entries from, and writes quick edits to, the active platform.
pub trait ListSource {
    /// Loads every entry on the active platform, most recently changed first.
    fn load(&self) -> impl Future<Output = Result<Vec<ListCard>, ListSourceError>> + Send;

    /// Applies `edit` to the entry for `media`.
    fn save(
        &self,
        media: MediaId,
        edit: &QuickEdit,
    ) -> impl Future<Output = Result<(), ListSourceError>> + Send;
}

/// The observable state of My Lists, with no GPUI or platform dependency.
#[derive(Debug, Clone)]
pub struct ListsSession<S> {
    source: S,
    cards: Vec<ListCard>,
    tab: ListTab,
    query: String,
    sort: ListSort,
}

impl<S> ListsSession<S> {
    /// An empty session using `source`.
    pub fn new(source: S) -> Self {
        Self {
            source,
            cards: Vec::new(),
            tab: ListTab::default(),
            query: String::new(),
            sort: ListSort::default(),
        }
    }

    /// The selected tab.
    pub fn tab(&self) -> &ListTab {
        &self.tab
    }

    /// The inline search text.
    pub fn query(&self) -> &str {
        &self.query
    }

    /// The chosen sort order.
    pub fn sort(&self) -> ListSort {
        self.sort
    }

    /// Every loaded card, before tab and query filtering.
    pub fn cards(&self) -> &[ListCard] {
        &self.cards
    }

    /// Selects another tab.
    pub fn set_tab(&mut self, tab: ListTab) {
        self.tab = tab;
    }

    /// Changes the inline search text.
    pub fn set_query(&mut self, query: String) {
        self.query = query;
    }

    /// Changes the sort order.
    pub fn set_sort(&mut self, sort: ListSort) {
        self.sort = sort;
    }

    /// Every status tab in platform order, then the custom lists that have
    /// entries. Status tabs stay present even when empty: the selected tab
    /// must never disagree with the highlighted tab, and a quick edit that
    /// empties a status must not make its tab disappear.
    pub fn tabs(&self) -> Vec<ListTab> {
        let mut custom: Vec<String> = self
            .cards
            .iter()
            .flat_map(|card| card.entry.custom_lists.iter().cloned())
            .collect();
        custom.sort();
        custom.dedup();
        STATUS_ORDER
            .into_iter()
            .map(ListTab::Status)
            .chain(custom.into_iter().map(ListTab::Custom))
            .collect()
    }

    /// Cards in the selected tab matching the query, in the chosen order.
    pub fn visible(&self) -> Vec<&ListCard> {
        let needle = self.query.trim().to_lowercase();
        let mut cards: Vec<&ListCard> = self
            .cards
            .iter()
            .filter(|card| {
                let in_tab = match &self.tab {
                    ListTab::Status(status) => {
                        card.entry.status == *status && !card.entry.hidden_from_status_lists
                    }
                    ListTab::Custom(name) => {
                        card.entry.custom_lists.iter().any(|list| list == name)
                    }
                };
                in_tab && (needle.is_empty() || card.title.to_lowercase().contains(&needle))
            })
            .collect();
        match self.sort {
            ListSort::Recent => cards.sort_by(|a, b| {
                b.entry
                    .updated_at
                    .cmp(&a.entry.updated_at)
                    .then_with(|| a.entry.media.cmp(&b.entry.media))
            }),
            ListSort::Title => cards.sort_by(|a, b| {
                a.title
                    .to_lowercase()
                    .cmp(&b.title.to_lowercase())
                    .then_with(|| a.entry.media.cmp(&b.entry.media))
            }),
            ListSort::Progress => cards.sort_by(|a, b| {
                b.entry
                    .progress
                    .cmp(&a.entry.progress)
                    .then_with(|| a.entry.media.cmp(&b.entry.media))
            }),
            ListSort::Score => cards.sort_by(|a, b| {
                b.entry
                    .score
                    .raw()
                    .cmp(&a.entry.score.raw())
                    .then_with(|| a.entry.media.cmp(&b.entry.media))
            }),
        }
        cards
    }
}

impl<S: ListSource> ListsSession<S> {
    /// Reloads entries from the active platform.
    ///
    /// # Errors
    ///
    /// Returns the source's error if the platform cannot be read.
    pub async fn reload(&mut self) -> Result<(), ListSourceError> {
        self.cards = self.source.load().await?;
        Ok(())
    }

    /// Saves a quick edit and reloads so the card reflects what was stored.
    ///
    /// # Errors
    ///
    /// Returns the source's error if the edit cannot be saved or the entries
    /// cannot be reloaded.
    pub async fn quick_edit(
        &mut self,
        media: MediaId,
        edit: &QuickEdit,
    ) -> Result<(), ListSourceError> {
        self.source.save(media, edit).await?;
        self.reload().await
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, SystemTime};

    use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, Score};

    use super::{
        ListCard, ListSort, ListSource, ListSourceError, ListTab, ListsSession, QuickEdit,
    };

    fn entry(id: i64, status: ListStatus, progress: u32) -> ListEntry {
        ListEntry {
            media: MediaId::new(id),
            status,
            score: Score::UNSCORED,
            progress,
            progress_volumes: 0,
            repeat: 0,
            started_at: FuzzyDate::UNKNOWN,
            completed_at: FuzzyDate::UNKNOWN,
            notes: String::new(),
            private: false,
            priority: 0,
            advanced_scores: AdvancedScores::default(),
            custom_lists: Vec::new(),
            hidden_from_status_lists: false,
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    fn card(id: i64, status: ListStatus, progress: u32, score: u8, title: &str) -> ListCard {
        let mut entry = entry(id, status, progress);
        entry.score = Score::new(score).unwrap_or(Score::UNSCORED);
        ListCard {
            entry,
            title: title.to_owned(),
            poster: None,
        }
    }

    struct FakeSource {
        cards: Arc<Mutex<Vec<ListCard>>>,
    }

    impl ListSource for FakeSource {
        async fn load(&self) -> Result<Vec<ListCard>, ListSourceError> {
            tokio::task::yield_now().await;
            let cards = self
                .cards
                .lock()
                .map_err(|_| ListSourceError::new("poisoned fake source"))?;
            Ok(cards.clone())
        }

        async fn save(&self, media: MediaId, edit: &QuickEdit) -> Result<(), ListSourceError> {
            tokio::task::yield_now().await;
            let mut cards = self
                .cards
                .lock()
                .map_err(|_| ListSourceError::new("poisoned fake source"))?;
            let stored = cards
                .iter_mut()
                .find(|card| card.entry.media == media)
                .ok_or_else(|| ListSourceError::new("unknown media"))?;
            if let Some(status) = edit.status {
                stored.entry.status = status;
            }
            if let Some(progress) = edit.progress {
                stored.entry.progress = progress;
            }
            if let Some(score) = edit.score {
                stored.entry.score = score;
            }
            Ok(())
        }
    }

    #[tokio::test]
    async fn status_and_custom_tabs_group_entries() {
        let mut current = card(1, ListStatus::Current, 3, 0, "One");
        current.entry.custom_lists = vec!["Favourites".to_owned()];
        let mut hidden = card(2, ListStatus::Current, 1, 0, "Two");
        hidden.entry.hidden_from_status_lists = true;
        hidden.entry.custom_lists = vec!["Favourites".to_owned()];
        let planned = card(3, ListStatus::Planning, 0, 0, "Three");
        let source = FakeSource {
            cards: Arc::new(Mutex::new(vec![current, hidden, planned])),
        };
        let mut session = ListsSession::new(source);
        session.reload().await.expect("fake source loads");

        assert_eq!(
            session.tabs(),
            vec![
                ListTab::Status(ListStatus::Current),
                ListTab::Status(ListStatus::Repeating),
                ListTab::Status(ListStatus::Planning),
                ListTab::Status(ListStatus::Completed),
                ListTab::Status(ListStatus::Paused),
                ListTab::Status(ListStatus::Dropped),
                ListTab::Custom("Favourites".to_owned()),
            ],
            "every status tab is present, then custom lists by name"
        );
        session.set_tab(ListTab::Status(ListStatus::Current));
        assert_eq!(
            session
                .visible()
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            vec!["One"],
            "hidden entries stay out of status tabs"
        );
        session.set_tab(ListTab::Custom("Favourites".to_owned()));
        assert_eq!(
            session.visible().len(),
            2,
            "custom lists include hidden entries"
        );
    }

    #[tokio::test]
    async fn search_filters_titles_case_insensitively() {
        let source = FakeSource {
            cards: Arc::new(Mutex::new(vec![
                card(1, ListStatus::Current, 0, 0, "Frieren"),
                card(2, ListStatus::Current, 0, 0, "Vinland Saga"),
            ])),
        };
        let mut session = ListsSession::new(source);
        session.reload().await.expect("fake source loads");
        session.set_tab(ListTab::Status(ListStatus::Current));
        session.set_query("frie".to_owned());

        assert_eq!(
            session
                .visible()
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Frieren"],
            "a partial, differently-cased query matches one card"
        );
    }

    #[tokio::test]
    async fn sort_orders_by_the_chosen_field() {
        let source = FakeSource {
            cards: Arc::new(Mutex::new(vec![
                card(1, ListStatus::Current, 2, 40, "Zebra"),
                card(2, ListStatus::Current, 9, 80, "Alpha"),
            ])),
        };
        let mut session = ListsSession::new(source);
        session.reload().await.expect("fake source loads");
        session.set_tab(ListTab::Status(ListStatus::Current));

        session.set_sort(ListSort::Title);
        assert_eq!(
            session
                .visible()
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Alpha", "Zebra"],
            "title sort is case-insensitive alphabetical"
        );
        session.set_sort(ListSort::Progress);
        assert_eq!(
            session
                .visible()
                .iter()
                .map(|card| card.entry.media.get())
                .collect::<Vec<_>>(),
            vec![2, 1],
            "progress sort is descending"
        );
        session.set_sort(ListSort::Score);
        assert_eq!(
            session
                .visible()
                .iter()
                .map(|card| card.entry.media.get())
                .collect::<Vec<_>>(),
            vec![2, 1],
            "score sort is descending"
        );
    }

    #[tokio::test]
    async fn an_empty_status_tab_stays_selected() {
        let source = FakeSource {
            cards: Arc::new(Mutex::new(vec![card(
                1,
                ListStatus::Planning,
                0,
                0,
                "Planned",
            )])),
        };
        let mut session = ListsSession::new(source);
        session.reload().await.expect("fake source loads");
        session.set_tab(ListTab::Status(ListStatus::Current));

        assert!(
            session
                .tabs()
                .contains(&ListTab::Status(ListStatus::Current)),
            "the Current tab exists even with no Current entries"
        );
        assert_eq!(session.tab(), &ListTab::Status(ListStatus::Current));
        assert!(session.visible().is_empty(), "its grid is empty");
    }

    #[tokio::test]
    async fn quick_edit_that_empties_a_tab_keeps_it_selected() {
        let cards = Arc::new(Mutex::new(vec![card(1, ListStatus::Current, 4, 0, "One")]));
        let mut session = ListsSession::new(FakeSource {
            cards: Arc::clone(&cards),
        });
        session.reload().await.expect("fake source loads");
        session
            .quick_edit(
                MediaId::new(1),
                &QuickEdit::new().status(ListStatus::Completed),
            )
            .await
            .expect("quick edit saves");

        assert_eq!(
            session.tab(),
            &ListTab::Status(ListStatus::Current),
            "the tab stays selected"
        );
        assert!(
            session
                .tabs()
                .contains(&ListTab::Status(ListStatus::Current)),
            "the emptied tab is still offered"
        );
        assert!(session.visible().is_empty(), "the grid shows no stale card");
    }

    #[tokio::test]
    async fn quick_edit_updates_the_card_and_reloads_from_the_source() {
        let cards = Arc::new(Mutex::new(vec![card(1, ListStatus::Planning, 0, 0, "One")]));
        let edit = QuickEdit::new()
            .status(ListStatus::Current)
            .progress(7)
            .score(Score::new(85).expect("valid score"));
        {
            let mut session = ListsSession::new(FakeSource {
                cards: Arc::clone(&cards),
            });
            session.reload().await.expect("fake source loads");
            session
                .quick_edit(MediaId::new(1), &edit)
                .await
                .expect("quick edit saves");
            session.set_tab(ListTab::Status(ListStatus::Current));
            let visible = session.visible();
            let card = visible.first().expect("edited card is visible");
            assert_eq!(card.entry.progress, 7, "card shows the new progress");
            assert_eq!(card.entry.status, ListStatus::Current, "card moved tabs");
        }

        let mut restarted = ListsSession::new(FakeSource { cards });
        restarted
            .reload()
            .await
            .expect("source persists across restart");
        let card = restarted.cards.first().expect("entry survives a restart");
        assert_eq!(card.entry.progress, 7, "progress survives a restart");
        assert_eq!(card.entry.status, ListStatus::Current, "status survives");
        assert_eq!(card.entry.score.raw(), 85, "score survives a restart");
    }

    #[tokio::test]
    async fn the_default_sort_puts_the_most_recent_first() {
        let mut older = card(1, ListStatus::Current, 0, 0, "Older");
        older.entry.updated_at = SystemTime::UNIX_EPOCH;
        let mut newer = card(2, ListStatus::Current, 0, 0, "Newer");
        newer.entry.updated_at = SystemTime::UNIX_EPOCH + Duration::from_secs(60);
        let source = FakeSource {
            cards: Arc::new(Mutex::new(vec![older, newer])),
        };
        let mut session = ListsSession::new(source);
        session.reload().await.expect("fake source loads");
        session.set_tab(ListTab::Status(ListStatus::Current));

        assert_eq!(session.sort(), ListSort::Recent, "recent is the default");
        assert_eq!(
            session
                .visible()
                .iter()
                .map(|card| card.title.as_str())
                .collect::<Vec<_>>(),
            vec!["Newer", "Older"],
            "most recently updated card comes first"
        );
    }
}
