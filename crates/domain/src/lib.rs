//! Domain types shared by every Autumn crate: ids, media models and events.
//!
//! No I/O lives here. See ARCHITECTURE §2.

use std::time::{Instant, SystemTime};

mod account;
mod fuzzy_date;
mod info_hash;
mod list_entry;
mod media;
mod playback;
mod score;
mod transfer;

pub use account::{ANILIST_RELOGIN_WINDOW, AniListAccountState};
pub use fuzzy_date::{FuzzyDate, FuzzyDateError};
pub use info_hash::{InfoHash, InfoHashError};
pub use list_entry::{AdvancedScores, ListEntry, ListStatus};
pub use media::{ExternalId, MediaId, MediaKind};
pub use playback::{
    AutoNext, ChapterKind, PlaybackChapter, PlaybackControl, PlaybackDetails, PlaybackState,
    StreamSource,
};
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
    /// A scan ended. The counts feed the summary toast; `error` is set when
    /// the scan failed before it could write a summary.
    LibraryScanFinished {
        /// Files found by the walk.
        files_walked: usize,
        /// Files matched to a media.
        files_matched: usize,
        /// Files left without a media.
        files_unmatched: usize,
        /// Files on a disconnected drive.
        shelved: usize,
        /// Why the scan failed, when it did.
        error: Option<String>,
    },
    /// A list transfer preview is ready for review, or `None` when dismissed.
    TransferPreview(Option<TransferPreview>),
    /// A background list transfer processed another entry.
    TransferProgress(TransferProgress),
    /// The one active session's position changed.
    PlaybackState(PlaybackState),
    /// The chapters of the playing file and the current chapter, if any.
    PlaybackChapters {
        /// The chapter marks in playback order.
        chapters: Vec<PlaybackChapter>,
        /// The chapter the player is in, when one is selected.
        current: Option<usize>,
    },
    /// The next-episode countdown changed, or `None` when it ended or was
    /// cancelled.
    PlaybackAutoNext(Option<AutoNext>),
    /// The session ended, so the HUD has nothing to show.
    PlaybackStopped {
        /// Whether the player quit on its own rather than through a command.
        unexpected: bool,
    },
    /// Progress sync failed; the UI can offer Retry when it is transient.
    PlaybackError {
        /// The media whose progress was not saved.
        media: MediaId,
        /// The episode whose progress was not saved.
        episode: u32,
        /// A message suitable for display in a toast.
        message: String,
        /// Whether a Retry action can succeed.
        retryable: bool,
    },
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
