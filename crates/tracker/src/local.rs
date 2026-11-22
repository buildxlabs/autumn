use db::Db;
use domain::{ListEntry, MediaId, MediaKind};

use crate::{Collection, Platform, PlatformError};

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
    async fn collection(&self, kind: MediaKind) -> Result<Collection, PlatformError> {
        let entries = self.db.local_list().entries(kind).await?;
        Ok(Collection::group(&entries))
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        Ok(self.db.local_list().save(entry).await?)
    }

    async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, PlatformError> {
        Ok(self.db.local_list().set_progress(media, progress).await?)
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), PlatformError> {
        self.db.local_list().delete(media).await?;
        Ok(())
    }
}
