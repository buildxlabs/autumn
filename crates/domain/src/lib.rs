//! Domain types shared by every Autumn crate: ids, media models and events.
//!
//! No I/O lives here. See ARCHITECTURE §2.

use std::time::{Instant, SystemTime};

mod account;
mod fuzzy_date;
mod list_entry;
mod media;
mod score;
mod transfer;

pub use account::{ANILIST_RELOGIN_WINDOW, AniListAccountState};
pub use fuzzy_date::{FuzzyDate, FuzzyDateError};
pub use list_entry::{AdvancedScores, ListEntry, ListStatus};
pub use media::{ExternalId, MediaId, MediaKind};
pub use score::{Score, ScoreError};
pub use transfer::{
    TransferChoice, TransferDirection, TransferItem, TransferPreview, TransferProgress,
    TransferState,
};

/// An event the engine publishes for the UI to apply to its stores.
///
/// Events travel on the bounded channel described in ARCHITECTURE §4.
/// Each engine module adds its own variants as it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppEvent {
    /// Stand-in for testing event delivery.
    Placeholder,
    /// The start of the oldest request, or no requests in flight.
    FetchActivity(Option<Instant>),
    /// When a queued AniList request may start, or the lockout expires.
    AniListRateLimit(Option<AniListWait>),
    /// The fetch state of one section, independent of the rest of the route.
    SectionFetch {
        /// The stable identifier of the section that made the request.
        section: &'static str,
        /// The outcome to render for that section.
        state: SectionFetch,
    },
    /// The account state changed; stores replace what they show.
    AccountChanged(AniListAccountState),
    /// An account action failed. The message is suitable for display.
    AccountError(String),
    /// A library scan made progress; the engine throttles these to 4 Hz.
    LibraryScanProgress {
        /// Files walked so far.
        files_walked: usize,
        /// Files matched so far.
        files_matched: usize,
    },
    /// A library scan finished and the local files may have changed.
    LibraryChanged,
    /// A list transfer preview is ready for review, or `None` when dismissed.
    TransferPreview(Option<TransferPreview>),
    /// A background list transfer processed another entry.
    TransferProgress(TransferProgress),
}

/// The next opportunity to start an AniList request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AniListWait {
    /// When another request may start.
    pub until: Instant,
    /// A request is waiting for capacity.
    pub queued: bool,
}

impl AniListWait {
    /// A deadline without a queued request.
    pub fn lockout(until: Instant) -> Self {
        Self {
            until,
            queued: false,
        }
    }

    /// A deadline while there is at least one queued request.
    pub fn queued(until: Instant) -> Self {
        Self {
            until,
            queued: true,
        }
    }
}

/// What a section can show after attempting a fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SectionFetch {
    /// The section can render its normal content.
    Ready,
    /// The request failed. The message is suitable for display to the user.
    Failed(String),
    /// AniList returned 403 and is temporarily unavailable.
    AniListUnavailable,
    /// The request was offline. A timestamp indicates cached content is available.
    Offline {
        /// When the cached section was last updated, if it exists.
        last_updated: Option<SystemTime>,
    },
}
