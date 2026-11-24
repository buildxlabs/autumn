//! The one active tracking platform, exposed to screens through ui-state
//! (ADR 0004). Screens never depend on the tracker crate directly.

use std::{
    collections::HashSet,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use db::Db;
use domain::{
    AniListAccountState, AppEvent, ListEntry, MediaId, MediaKind, TransferDirection,
    TransferPreview, TransferProgress,
};
use metadata::AniList;

use crate::events::Sender;
use tracker::{
    ActivePlatform, AniListPlatform, DbCheckpoints, ListTransfer, Local, Platform, PlatformError,
    TransferError, TransferOutcome,
};

/// Which platform reads and writes list entries right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ActiveKind {
    /// The account-free local platform.
    Local,
    /// The connected AniList account.
    AniList,
}

impl ActiveKind {
    // A private conversion, not a `From` impl: a public trait impl would put
    // a tracker type in ui-state's API.
    fn of(kind: tracker::ActiveKind) -> Self {
        match kind {
            tracker::ActiveKind::Local => Self::Local,
            tracker::ActiveKind::AniList => Self::AniList,
        }
    }
}

/// The kind of a gateway failure, for callers deciding whether to retry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum GatewayErrorKind {
    /// The platform state is poisoned by a panic on another thread.
    Unavailable,
    /// AniList rejected the token; the account is now read-only.
    Unauthorized,
    /// A write was attempted while AniList is read-only.
    ReadOnly,
    /// A list transfer needs a connected account.
    NotConnected,
    /// Any other platform failure.
    Other,
}

/// Why a platform request through the gateway failed.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct GatewayError {
    message: String,
    kind: GatewayErrorKind,
}

impl GatewayError {
    fn new(kind: GatewayErrorKind, message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            kind,
        }
    }

    /// The kind of failure, without exposing a platform crate's errors.
    #[must_use]
    pub fn kind(&self) -> GatewayErrorKind {
        self.kind
    }

    /// A message safe to show in the UI.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Whether the UI should offer a Retry action for this failure.
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        !matches!(
            self.kind,
            GatewayErrorKind::Unauthorized
                | GatewayErrorKind::ReadOnly
                | GatewayErrorKind::NotConnected
        )
    }
}

/// An opaque handle to the connected AniList account. The tracker crate
/// builds the concrete platform; callers never name it.
pub struct AniListHandle {
    inner: AniListPlatform,
}

impl fmt::Debug for AniListHandle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AniListHandle").finish_non_exhaustive()
    }
}

impl AniListHandle {
    /// Uses `client` and `token` for every AniList request and keeps the media
    /// cross-references in `db`.
    #[must_use]
    pub fn new(db: Db, client: AniList, token: String, user_name: String) -> Self {
        Self {
            inner: AniListPlatform::new(db, client, token, user_name),
        }
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
    db: Db,
}

/// The two platform orders a transfer can take. `Platform` is not object
/// safe, so both concrete engines live here rather than behind a trait object.
enum GatewayTransfer {
    ToAniList(ListTransfer<Local, AniListPlatform, DbCheckpoints>),
    ToLocal(ListTransfer<AniListPlatform, Local, DbCheckpoints>),
}

impl GatewayTransfer {
    async fn preview(&self) -> Result<TransferPreview, TransferError> {
        match self {
            Self::ToAniList(transfer) => transfer.preview().await,
            Self::ToLocal(transfer) => transfer.preview().await,
        }
    }

    async fn apply(
        &self,
        preview: &TransferPreview,
        progress: impl FnMut(TransferProgress),
    ) -> Result<TransferOutcome, TransferError> {
        match self {
            Self::ToAniList(transfer) => transfer.apply(preview, progress).await,
            Self::ToLocal(transfer) => transfer.apply(preview, progress).await,
        }
    }
}

impl PlatformGateway {
    /// Starts with the account-free Local platform over `db` active.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self::from_local(db.clone(), Local::new(db), None)
    }

    /// A gateway that publishes a rejected token as an expired account.
    #[must_use]
    pub fn with_events(db: Db, events: Sender) -> Self {
        Self::from_local(db.clone(), Local::new(db), Some(events))
    }

    fn from_local(db: Db, local: Local, events: Option<Sender>) -> Self {
        Self {
            active: Arc::new(Mutex::new(ActivePlatform::new(local))),
            generation: Arc::new(AtomicU64::new(0)),
            read_only: Arc::new(AtomicBool::new(false)),
            events,
            db,
        }
    }

    /// A gateway over the account-free Local platform backed by `db`.
    #[must_use]
    pub fn local(db: Db) -> Self {
        Self::new(db)
    }

    /// Whether AniList is read-only after a rejected token.
    #[must_use]
    pub fn is_read_only(&self) -> bool {
        self.read_only.load(Ordering::SeqCst)
    }

    fn platform(&self) -> Result<ActivePlatform, GatewayError> {
        self.active
            .lock()
            .map_err(|_| {
                GatewayError::new(
                    GatewayErrorKind::Unavailable,
                    "the active platform is unavailable",
                )
            })
            .map(|active| active.clone())
    }

    fn writable_platform(&self) -> Result<ActivePlatform, GatewayError> {
        if self.is_read_only() {
            return Err(GatewayError::new(
                GatewayErrorKind::ReadOnly,
                "AniList is read-only until you log in again",
            ));
        }
        self.platform()
    }

    fn classify<T>(&self, result: Result<T, PlatformError>) -> Result<T, GatewayError> {
        match result {
            Ok(value) => Ok(value),
            Err(PlatformError::Unauthorized) => {
                self.expire();
                Err(GatewayError::new(
                    GatewayErrorKind::Unauthorized,
                    "AniList rejected the token; lists are read-only",
                ))
            }
            Err(error) => Err(GatewayError::new(
                GatewayErrorKind::Other,
                error.to_string(),
            )),
        }
    }

    /// Marks AniList read-only at startup for a restored expired account,
    /// without emitting a second account event.
    pub fn mark_read_only(&self) {
        self.read_only.store(true, Ordering::SeqCst);
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
            .map_or(ActiveKind::Local, |active| ActiveKind::of(active.active()))
    }

    /// Connects AniList and makes it active; Local entries stay dormant.
    pub fn connect_anilist(&self, handle: AniListHandle) {
        self.read_only.store(false, Ordering::SeqCst);
        if let Ok(mut active) = self.active.lock() {
            active.connect(handle.inner);
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
        let platform = self.platform()?;
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

    /// Updates progress on the active platform and returns the entry as
    /// stored, which may differ from the request (AniList can complete an
    /// entry on its last episode).
    ///
    /// # Errors
    ///
    /// Returns a gateway error when the platform rejects the write.
    pub async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, GatewayError> {
        let platform = self.writable_platform()?;
        self.classify(platform.update_progress(media, progress).await)
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

    /// Whether a list transfer can be previewed: AniList must still be
    /// connected so both sides of the transfer can be read.
    #[must_use]
    pub fn can_transfer(&self) -> bool {
        self.platform()
            .is_ok_and(|active| active.anilist().is_some())
    }

    fn transfer(&self, direction: TransferDirection) -> Result<GatewayTransfer, GatewayError> {
        let active = self.platform()?;
        let Some(anilist) = active.anilist() else {
            return Err(GatewayError::new(
                GatewayErrorKind::NotConnected,
                "connect AniList before transferring lists",
            ));
        };
        let local = active.local();
        let checkpoints = DbCheckpoints::new(self.db.clone());
        Ok(match direction {
            TransferDirection::ToAniList => GatewayTransfer::ToAniList(ListTransfer::new(
                direction,
                local,
                anilist,
                checkpoints,
            )),
            TransferDirection::ToLocal => {
                GatewayTransfer::ToLocal(ListTransfer::new(direction, anilist, local, checkpoints))
            }
        })
    }

    fn classify_transfer<T>(&self, result: Result<T, TransferError>) -> Result<T, GatewayError> {
        match result {
            Ok(value) => Ok(value),
            Err(TransferError::Platform(error)) => self.classify(Err(error)),
            _ => Err(GatewayError::new(
                GatewayErrorKind::Other,
                "the list transfer failed",
            )),
        }
    }

    /// Builds the preview for `direction` from both platforms. Identical
    /// entries and entries confirmed by an unfinished run are left out.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when either platform cannot be read.
    pub async fn transfer_preview(
        &self,
        direction: TransferDirection,
    ) -> Result<TransferPreview, GatewayError> {
        self.classify_transfer(self.transfer(direction)?.preview().await)
    }

    /// Applies the reviewed transfer in the background, reporting one progress
    /// snapshot per entry. A rejected token makes the gateway read-only.
    ///
    /// # Errors
    ///
    /// Returns a gateway error when a write or checkpoint fails; the saved
    /// checkpoint lets the next run resume.
    pub async fn transfer_apply(
        &self,
        direction: TransferDirection,
        preview: &TransferPreview,
        progress: impl FnMut(TransferProgress),
    ) -> Result<TransferOutcome, GatewayError> {
        self.classify_transfer(self.transfer(direction)?.apply(preview, progress).await)
    }
}

#[cfg(test)]
mod tests {
    use std::time::SystemTime;

    use domain::{
        AdvancedScores, AniListAccountState, AppEvent, ExternalId, FuzzyDate, ListEntry,
        ListStatus, MediaId, MediaKind, Score,
    };
    use tracker::{Local, Platform};

    use super::{ActiveKind, AniListHandle, GatewayError, GatewayErrorKind, PlatformGateway};

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

        let gateway = PlatformGateway::new(db.clone());
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
        gateway.connect_anilist(AniListHandle::new(
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
        let gateway = PlatformGateway::with_events(db, sender);

        let result: Result<(), tracker::PlatformError> = Err(tracker::PlatformError::Unauthorized);
        let classified = gateway.classify(result);

        assert!(matches!(classified, Err(error) if error.kind() == GatewayErrorKind::Unauthorized));
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
        assert_eq!(error.kind(), GatewayErrorKind::ReadOnly);
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
        let kind = |kind| GatewayError::new(kind, "test error");
        assert!(kind(GatewayErrorKind::Unavailable).is_retryable());
        assert!(kind(GatewayErrorKind::Other).is_retryable());
        assert!(!kind(GatewayErrorKind::Unauthorized).is_retryable());
        assert!(!kind(GatewayErrorKind::ReadOnly).is_retryable());
    }
}
