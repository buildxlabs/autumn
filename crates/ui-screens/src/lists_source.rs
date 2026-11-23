//! The account-free Local platform behind My Lists (ticket #60).
//!
//! Entries come from SQLite and their card text from the stale-first catalog
//! cache. Reading the cache never fetches, so this screen makes zero network
//! requests while the Local platform is active.

use db::Db;
use domain::{ExternalId, ListEntry, MediaId, MediaKind};
use metadata::CatalogCache;
use ui_state::{ListCard, ListSource, ListSourceError, QuickEdit};

use crate::palette_search::display_label;

fn source_error(error: impl std::fmt::Display) -> ListSourceError {
    ListSourceError::new(error.to_string())
}

/// Reads and writes list entries on the Local platform.
#[derive(Debug, Clone)]
pub(crate) struct LocalListSource {
    db: Db,
    catalog: CatalogCache,
}

impl LocalListSource {
    /// Uses `db` for entries and `catalog` for cached card text only.
    pub(crate) fn new(db: Db, catalog: CatalogCache) -> Self {
        Self { db, catalog }
    }

    /// The title and poster already on disk for `entry`, or a fallback.
    async fn card(&self, entry: ListEntry) -> Result<ListCard, ListSourceError> {
        let ids = self
            .db
            .media()
            .external_ids(entry.media)
            .await
            .map_err(source_error)?;
        let anilist_id = ids.iter().find_map(|id| match id {
            ExternalId::AniList(id) => Some(*id),
            _ => None,
        });
        let (title, poster) = match anilist_id {
            Some(id) => match self.catalog.cached_media(id).await {
                Some(cached) => (
                    display_label(&cached.value.summary),
                    cached.value.summary.poster.clone(),
                ),
                None => (format!("Media #{id}"), None),
            },
            None => (format!("Media #{}", entry.media.get()), None),
        };
        Ok(ListCard {
            entry,
            title,
            poster,
        })
    }
}

impl ListSource for LocalListSource {
    async fn load(&self) -> Result<Vec<ListCard>, ListSourceError> {
        let mut cards = Vec::new();
        for kind in [MediaKind::Anime, MediaKind::Manga] {
            let entries = self
                .db
                .local_list()
                .entries(kind)
                .await
                .map_err(source_error)?;
            for entry in entries {
                cards.push(self.card(entry).await?);
            }
        }
        Ok(cards)
    }

    async fn save(&self, media: MediaId, edit: &QuickEdit) -> Result<(), ListSourceError> {
        let mut entry = self
            .db
            .local_list()
            .entry(media)
            .await
            .map_err(source_error)?
            .ok_or_else(|| ListSourceError::new("no list entry for that media"))?;
        if let Some(status) = edit.status {
            entry.status = status;
        }
        if let Some(progress) = edit.progress {
            entry.progress = progress;
        }
        if let Some(score) = edit.score {
            entry.score = score;
        }
        self.db
            .local_list()
            .save(&entry)
            .await
            .map_err(source_error)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use db::Db;
    use domain::{
        AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
    };
    use ui_state::{ListTab, ListsSession, QuickEdit};

    use super::LocalListSource;

    fn entry(media: MediaId, status: ListStatus, progress: u32) -> ListEntry {
        ListEntry {
            media,
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

    fn source(db: Db, cache_dir: &std::path::Path) -> LocalListSource {
        let http = net::Client::builder()
            .max_retries(0)
            .build()
            .expect("test client");
        let anilist = metadata::AniList::new(http);
        let disk = net::PosterCache::new(cache_dir).expect("test cache dir");
        LocalListSource::new(db, metadata::CatalogCache::new(anilist, disk))
    }

    #[tokio::test]
    async fn a_quick_edit_is_stored_in_sqlite_and_survives_a_restart() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db_path = dir.path().join("autumn.db");
        let media = {
            let db = Db::open(&db_path).await.expect("open db");
            let minted = db
                .media()
                .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
                .await
                .expect("mint media");
            db.local_list()
                .save(&entry(minted.media, ListStatus::Planning, 0))
                .await
                .expect("save entry");
            minted.media
        };

        let edit = QuickEdit::new()
            .status(ListStatus::Current)
            .progress(9)
            .score(Score::new(75).expect("valid score"));
        {
            let db = Db::open(&db_path).await.expect("reopen db");
            let mut session = ListsSession::new(source(db, dir.path()));
            session.reload().await.expect("load entries");
            session.quick_edit(media, &edit).await.expect("save edit");
            session.set_tab(ListTab::Status(ListStatus::Current));
            let visible = session.visible();
            let card = visible.first().expect("edited entry is in the new tab");
            assert_eq!(card.entry.progress, 9, "card shows saved progress");
        }

        let db = Db::open(&db_path).await.expect("reopen db after restart");
        let mut restarted = ListsSession::new(source(db, dir.path()));
        restarted
            .reload()
            .await
            .expect("load entries after restart");
        let card = restarted.cards().first().expect("entry survives restart");
        assert_eq!(card.entry.status, ListStatus::Current, "status persists");
        assert_eq!(card.entry.progress, 9, "progress persists");
        assert_eq!(card.entry.score.raw(), 75, "score persists");
    }
}
