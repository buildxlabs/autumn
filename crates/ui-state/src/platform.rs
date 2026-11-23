//! The one active tracking platform, exposed to screens through ui-state
//! (ADR 0004). Screens never depend on the tracker crate directly.

use std::{
    collections::HashSet,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use db::Db;
use domain::{AniListAccountState, AppEvent, ListEntry, MediaId, MediaKind};

use crate::events::Sender;
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
    /// AniList rejected the token; the account is now read-only.
    #[error("AniList rejected the token; lists are read-only")]
    Unauthorized,
    /// A write was attempted while AniList is read-only.
    #[error("AniList is read-only until you log in again")]
    ReadOnly,
}

impl GatewayError {
    /// Whether the UI should offer a Retry action for this failure.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        !matches!(self, Self::Unauthorized | Self::ReadOnly)
    }
}

/// A cheap handle to the active platform, shared by every screen.
///
/// The concrete platforms are cloned out of a short lock before any await,
/// so no mutex guard is held across I/O.
#[derive(Debug, Clone)]
pub struct PlatformGateway {
    active: Arc<Mutex<ActivePlatform>>,
    generation: Arc<AtomicU64>,
    read_only: Arc<AtomicBool>,
    events: Option<Sender>,
}

impl PlatformGateway {
    /// Starts with `local` active and no AniList account connected.
    #[must_use]
    pub fn new(local: Local) -> Self {
        Self {
            active: Arc::new(Mutex::new(ActivePlatform::new(local))),
            generation: Arc::new(AtomicU64::new(0)),
            read_only: Arc::new(AtomicBool::new(false)),
            events: None,
        }
    }

    /// A gateway that publishes a rejected token as an expired account.
    #[must_use]
    pub fn with_events(local: Local, events: Sender) -> Self {
        Self {
            events: Some(events),
            ..Self::new(local)
        }
    }

    /// A gateway over the account-free Local platform backed by `db`.
    #[must_use]
    pub fn local(db: Db) -> Self {
        Self::new(Local::new(db))
    }

    /// Whether AniList is read-only after a rejected token.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    fn writable_platform(&self) -> Result<ActivePlatform, GatewayError> {
        if self.is_read_only() {
            return Err(GatewayError::ReadOnly);
        }
        self.active
            .lock()
            .map_err(|_| GatewayError::Unavailable)
            .map(|active| active.clone())
    }

    fn classify<T>(&self, result: Result<T, PlatformError>) -> Result<T, GatewayError> {
        match result {
            Ok(value) => Ok(value),
            Err(PlatformError::Unauthorized) => {
                self.expire();
                Err(GatewayError::Unauthorized)
            }
            Err(error) => Err(GatewayError::Platform(error)),
        }
    }

    fn expire(&self) {
        self.read_only.store(true, Ordering::SeqCst);
        if let Some(events) = &self.events {
            let _ = events.send(AppEvent::AccountChanged(AniListAccountState::Expired {
                name: None,
                avatar: None,
            }));
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
        self.read_only.store(false, Ordering::SeqCst);
        if let Ok(mut active) = self.active.lock() {
            active.connect(anilist);
            self.generation.fetch_add(1, Ordering::SeqCst);
        }
    }

    /// Signs out of AniList and makes Local active again, untouched.
    pub fn sign_out(&self) {
        // An expired AniList token must not leave Local read-only after sign-out.
        self.read_only.store(false, Ordering::SeqCst);
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
        let collection = self.classify(platform.collection(kind).await)?;
        let mut seen = HashSet::new();
        let mut entries = Vec::new();
        for list in collection.lists {
            for entry in list.entries {
                if seen.insert(entry.media) {
                    entries.push(entry);
                }
            }
        }
        Ok(entries)
    }

    /// Saves a full list entry and returns it as stored, so a screen can patch
    /// one card without reloading the whole collection.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the write. A rejected
    /// token makes the gateway read-only and publishes an expired account.
    pub async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, GatewayError> {
        let platform = self.writable_platform()?;
        self.classify(platform.save_entry(entry).await)
    }

    /// Updates progress on the active platform.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the write.
    pub async fn update_progress(&self, media: MediaId, progress: u32) -> Result<(), GatewayError> {
        let platform = self.writable_platform()?;
        self.classify(platform.update_progress(media, progress).await)?;
        Ok(())
    }

    /// Deletes an entry on the active platform.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the delete.
    pub async fn delete_entry(&self, media: MediaId) -> Result<(), GatewayError> {
        let platform = self.writable_platform()?;
        self.classify(platform.delete_entry(media).await)
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use domain::{
        AdvancedScores, AniListAccountState, AppEvent, ExternalId, FuzzyDate, ListEntry,
        ListStatus, MediaId, MediaKind, Score,
    };
    use tracker::{ActiveKind, Local, Platform};

    use super::{GatewayError, PlatformGateway};

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

    #[tokio::test]
    async fn a_rejected_token_marks_the_gateway_read_only_and_publishes_an_expired_account() {
        let (sender, receiver) = crate::events::channel();
        let dir = tempfile::tempdir().expect("temp dir");
        let db = db::Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("db");
        let gateway = PlatformGateway::with_events(Local::new(db), sender);

        let result: Result<(), tracker::PlatformError> = Err(tracker::PlatformError::Unauthorized);
        let classified = gateway.classify(result);

        assert!(matches!(classified, Err(GatewayError::Unauthorized)));
        assert!(gateway.is_read_only(), "writes are refused from now on");
        assert!(
            matches!(
                receiver.try_recv(),
                Ok(AppEvent::AccountChanged(
                    AniListAccountState::Expired { .. }
                ))
            ),
            "the account store sees a read-only account"
        );
    }

    #[tokio::test]
    async fn a_read_only_gateway_refuses_writes_without_touching_the_platform() {
        let dir = tempfile::tempdir().expect("temp dir");
        let db = db::Db::open(&dir.path().join("autumn.db"))
            .await
            .expect("db");
        let minted = db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(154_587)])
            .await
            .expect("mint");
        let gateway = PlatformGateway::local(db);
        gateway.expire();

        let error = gateway
            .save_entry(&entry(minted.media, 9))
            .await
            .expect_err("read-only rejects the write");
        assert!(matches!(error, GatewayError::ReadOnly));
        assert!(!error.is_retryable(), "a read-only account offers no Retry");

        gateway.sign_out();
        assert!(!gateway.is_read_only(), "sign-out restores writable Local");
        gateway
            .save_entry(&entry(minted.media, 9))
            .await
            .expect("Local is writable again");
    }

    #[test]
    fn only_transient_gateway_errors_are_retryable() {
        assert!(GatewayError::Unavailable.is_retryable());
        assert!(!GatewayError::Unauthorized.is_retryable());
        assert!(!GatewayError::ReadOnly.is_retryable());
    }
}
