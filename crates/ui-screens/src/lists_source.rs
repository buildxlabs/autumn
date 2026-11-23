//! The active platform behind My Lists (tickets #60 and #66).
//!
//! Entries and edits go through ui-state's `PlatformGateway`, so this screen
//! never names the tracker crate. Card text comes from the stale-first cache
//! and only needs SQLite for external-id cross-references; a cache miss never
//! fetches, so the Local platform still makes zero network requests.

use db::Db;
use domain::{ExternalId, ListEntry, MediaId, MediaKind};
use metadata::CatalogCache;
use ui_state::{ListCard, ListSource, ListSourceError, PlatformGateway, QuickEdit};

use crate::palette_search::display_label;

fn source_error(error: impl std::fmt::Display) -> ListSourceError {
    ListSourceError::new(error.to_string())
}

/// Reads and writes list entries on whichever platform is active.
#[derive(Debug, Clone)]
pub(crate) struct PlatformListSource {
    gateway: PlatformGateway,
    db: Db,
    catalog: CatalogCache,
}

impl PlatformListSource {
    /// Uses `gateway` for entries and edits, `db` only for external ids and
    /// `catalog` for cached card text.
    pub(crate) fn new(gateway: PlatformGateway, db: Db, catalog: CatalogCache) -> Self {
        Self {
            gateway,
            db,
            catalog,
        }
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

impl ListSource for PlatformListSource {
    async fn load(&self) -> Result<Vec<ListCard>, ListSourceError> {
        let mut cards = Vec::new();
        for kind in [MediaKind::Anime, MediaKind::Manga] {
            for entry in self.gateway.entries(kind).await.map_err(source_error)? {
                cards.push(self.card(entry).await?);
            }
        }
        Ok(cards)
    }

    async fn save(&self, media: MediaId, edit: &QuickEdit) -> Result<(), ListSourceError> {
        self.gateway
            .edit_entry(media, edit)
            .await
            .map_err(source_error)
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use db::Db;
    use domain::{
        AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
    };
    use ui_state::{ListTab, ListsSession, PlatformGateway, QuickEdit};

    use super::PlatformListSource;

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

    fn source(db: Db, cache_dir: &std::path::Path) -> PlatformListSource {
        let http = net::Client::builder()
            .max_retries(0)
            .build()
            .expect("test client");
        let anilist = metadata::AniList::new(http);
        let disk = net::PosterCache::new(cache_dir).expect("test cache dir");
        let gateway = PlatformGateway::new(tracker::Local::new(db.clone()));
        PlatformListSource::new(gateway, db, metadata::CatalogCache::new(anilist, disk))
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

    #[tokio::test]
    async fn loading_entries_sends_no_anilist_requests() {
        use std::sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        };

        use axum::{Json, Router, extract::State, routing::post};
        use serde_json::Value;

        let hits = Arc::new(AtomicUsize::new(0));
        let app = Router::new()
            .route(
                "/",
                post(
                    move |State(hits): State<Arc<AtomicUsize>>, Json(_): Json<Value>| {
                        hits.fetch_add(1, Ordering::SeqCst);
                        async { Json(serde_json::json!({"data": {}})) }
                    },
                ),
            )
            .with_state(Arc::clone(&hits));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind mock");
        let address = listener.local_addr().expect("address");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });

        let dir = tempfile::tempdir().expect("temp dir");
        let db = Db::open(&dir.path().join("autumn.db")).await.expect("db");
        let minted = db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
            .await
            .expect("mint");
        db.local_list()
            .save(&entry(minted.media, ListStatus::Current, 3))
            .await
            .expect("save entry");

        let http = net::Client::builder()
            .max_retries(0)
            .build()
            .expect("http client");
        let anilist = metadata::AniList::with_endpoint(http, format!("http://{address}/"));
        let disk = net::PosterCache::new(dir.path()).expect("cache dir");
        let gateway = PlatformGateway::new(tracker::Local::new(db.clone()));
        let source =
            PlatformListSource::new(gateway, db, metadata::CatalogCache::new(anilist, disk));
        let mut session = ListsSession::new(source);
        session.reload().await.expect("load entries");

        assert_eq!(session.cards().len(), 1, "the entry was loaded");
        assert_eq!(hits.load(Ordering::SeqCst), 0, "Local made no requests");
    }
}
