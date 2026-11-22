use db::Db;
use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};

use crate::{Collection, Platform, TrackerError};

/// The account-free platform: list entries kept in the app database. It is
/// the active platform until the user connects a tracking account.
#[derive(Debug, Clone)]
pub struct Local {
    db: Db,
}

impl Local {
    /// The Local platform over `db`.
    pub fn new(db: Db) -> Self {
        Self { db }
    }
}

impl Platform for Local {
    async fn collection(&self, kind: MediaKind) -> Result<Collection, TrackerError> {
        let entries = self.db.local_list().entries(kind).await?;
        Ok(Collection::group(&entries))
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, TrackerError> {
        Ok(self.db.local_list().save(entry).await?)
    }

    async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, TrackerError> {
        let list = self.db.local_list();
        let entry = match list.entry(media).await? {
            Some(entry) => ListEntry { progress, ..entry },
            None => ListEntry {
                progress,
                ..current(media)
            },
        };
        Ok(list.save(&entry).await?)
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), TrackerError> {
        self.db.local_list().delete(media).await?;
        Ok(())
    }
}

/// A Current entry with nothing else set. Its timestamps are placeholders
/// that saving replaces.
fn current(media: MediaId) -> ListEntry {
    ListEntry {
        media,
        status: ListStatus::Current,
        score: Score::UNSCORED,
        progress: 0,
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
        created_at: std::time::SystemTime::UNIX_EPOCH,
        updated_at: std::time::SystemTime::UNIX_EPOCH,
    }
}
