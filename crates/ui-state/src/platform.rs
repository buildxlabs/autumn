//! The one active tracking platform, exposed to screens through ui-state
//! (ADR 0004). Screens never depend on the tracker crate directly.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU64, Ordering},
};

use domain::{ListEntry, MediaId, MediaKind};

use crate::lists::QuickEdit;
use tracker::{ActiveKind, ActivePlatform, AniListPlatform, Local, Platform, PlatformError};

/// Why a platform request through the gateway failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum GatewayError {
    /// The platform state is poisoned by a panic on another thread.
    #[error("the active platform is unavailable")]
    Unavailable,
    /// The active platform failed the operation.
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

/// A cheap handle to the active platform, shared by every screen.
///
/// The concrete platforms are cloned out of a short lock before any await,
/// so no mutex guard is held across I/O.
#[derive(Debug, Clone)]
pub struct PlatformGateway {
    active: Arc<Mutex<ActivePlatform>>,
    generation: Arc<AtomicU64>,
}

impl PlatformGateway {
    /// Starts with `local` active and no AniList account connected.
    #[must_use]
    pub fn new(local: Local) -> Self {
        Self {
            active: Arc::new(Mutex::new(ActivePlatform::new(local))),
            generation: Arc::new(AtomicU64::new(0)),
        }
    }

    /// Incremented whenever the active platform changes, so screens can
    /// reload without watching account events themselves.
    #[must_use]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    /// Which platform is active.
    #[must_use]
    pub fn active(&self) -> ActiveKind {
        self.active
            .lock()
            .map_or(ActiveKind::Local, |active| active.active())
    }

    /// Connects AniList and makes it active; Local entries stay dormant.
    pub fn connect_anilist(&self, anilist: AniListPlatform) {
        if let Ok(mut active) = self.active.lock() {
            active.connect(anilist);
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Signs out of AniList and makes Local active again, untouched.
    pub fn sign_out(&self) {
        if let Ok(mut active) = self.active.lock() {
            active.sign_out();
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// All list entries on the active platform, deduplicated across the
    /// status and custom-list groups AniList returns.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform cannot be read.
    pub async fn entries(&self, kind: MediaKind) -> Result<Vec<ListEntry>, GatewayError> {
        let platform = self
            .active
            .lock()
            .map_err(|_| GatewayError::Unavailable)?
            .clone();
        let collection = platform.collection(kind).await?;
        let mut seen = Vec::new();
        let mut entries = Vec::new();
        for list in collection.lists {
            for entry in list.entries {
                if !seen.contains(&entry.media) {
                    seen.push(entry.media);
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }

    /// Saves a full list entry on the active platform.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the write.
    pub async fn save_entry(&self, entry: &ListEntry) -> Result<(), GatewayError> {
        let platform = self
            .active
            .lock()
            .map_err(|_| GatewayError::Unavailable)?
            .clone();
        platform.save_entry(entry).await?;
        Ok(())
    }

    /// Applies a quick edit to the active platform's entry. A missing entry
    /// is a no-op, matching the platform trait's delete contract.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the entry cannot be read or saved.
    pub async fn edit_entry(&self, media: MediaId, edit: &QuickEdit) -> Result<(), GatewayError> {
        let platform = self
            .active
            .lock()
            .map_err(|_| GatewayError::Unavailable)?
            .clone();
        let mut found = None;
        for kind in [MediaKind::Anime, MediaKind::Manga] {
            if let Some(entry) = platform
                .collection(kind)
                .await?
                .lists
                .into_iter()
                .flat_map(|list| list.entries)
                .find(|entry| entry.media == media)
            {
                found = Some(entry);
                break;
            }
        }
        let Some(mut entry) = found else {
            return Ok(());
        };
        if let Some(status) = edit.status {
            entry.status = status;
        }
        if let Some(progress) = edit.progress {
            entry.progress = progress;
        }
        if let Some(score) = edit.score {
            entry.score = score;
        }
        platform.save_entry(&entry).await?;
        Ok(())
    }

    /// Updates progress on the active platform.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the write.
    pub async fn update_progress(&self, media: MediaId, progress: u32) -> Result<(), GatewayError> {
        let platform = self
            .active
            .lock()
            .map_err(|_| GatewayError::Unavailable)?
            .clone();
        platform.update_progress(media, progress).await?;
        Ok(())
    }

    /// Deletes an entry on the active platform.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the delete.
    pub async fn delete_entry(&self, media: MediaId) -> Result<(), GatewayError> {
        let platform = self
            .active
            .lock()
            .map_err(|_| GatewayError::Unavailable)?
            .clone();
        platform.delete_entry(media).await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use domain::{
        AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
    };
    use tracker::{ActiveKind, Local, Platform};

    use super::PlatformGateway;

    fn entry(media: MediaId, progress: u32) -> ListEntry {
        ListEntry {
            media,
            status: ListStatus::Current,
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

    #[tokio::test]
    async fn connecting_anilist_keeps_local_dormant_and_sign_out_restores_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = db::Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("db");
        let minted = db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
            .await
            .expect("mint");
        let local = Local::new(db.clone());
        local
            .save_entry(&entry(minted.media, 5))
            .await
            .expect("save");

        let gateway = PlatformGateway::new(local);
        assert_eq!(gateway.active(), ActiveKind::Local);
        let entries = gateway
            .entries(MediaKind::Anime)
            .await
            .expect("local entries");
        assert_eq!(
            entries.first().expect("entry").progress,
            5,
            "Local starts active"
        );

        let http = net::Client::builder().max_retries(0).build().expect("http");
        let anilist = metadata::AniList::with_endpoint(http, "http://127.0.0.1:1/");
        gateway.connect_anilist(tracker::AniListPlatform::new(
            db.clone(),
            anilist,
            "test-token".to_owned(),
            "TestUser".to_owned(),
        ));
        assert_eq!(gateway.active(), ActiveKind::AniList);
        assert_eq!(gateway.generation(), 1, "screens can see the switch");
        let dormant = db
            .local_list()
            .entry(minted.media)
            .await
            .expect("read")
            .expect("entry");
        assert_eq!(dormant.progress, 5, "AniList never touches Local");

        gateway.sign_out();
        assert_eq!(gateway.active(), ActiveKind::Local);
        assert_eq!(gateway.generation(), 2);
        let restored = gateway
            .entries(MediaKind::Anime)
            .await
            .expect("local entries");
        assert_eq!(
            restored.first().expect("entry").progress,
            5,
            "Local entries survive untouched"
        );
    }
}
