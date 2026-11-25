//! Vocabulary for a reviewed, one-shot list transfer (ADR 0004).
//!
//! A transfer copies list entries from one platform to another. It creates
//! and updates entries on the destination, never deletes them, and keeps the
//! destination's entry on conflict unless the user overrides that item.

use serde::{Deserialize, Serialize};

use crate::{ListEntry, MediaId, MediaKind};

/// Which way a list transfer copies: from Local to AniList on connect, or
/// from AniList to Local on sign-out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the two transfer directions are fixed by ADR 0004 and double as database keys"
)]
pub enum TransferDirection {
    /// Copy Local entries into the connected AniList account.
    ToAniList,
    /// Copy AniList entries into the dormant Local platform.
    ToLocal,
}

impl TransferDirection {
    /// The platform entries are copied from, for display.
    #[must_use]
    pub const fn source_label(self) -> &'static str {
        match self {
            Self::ToAniList => "Local",
            Self::ToLocal => "AniList",
        }
    }

    /// The platform entries are copied to, for display.
    #[must_use]
    pub const fn destination_label(self) -> &'static str {
        match self {
            Self::ToAniList => "AniList",
            Self::ToLocal => "Local",
        }
    }
}

/// What to do with one entry in a reviewed transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TransferChoice {
    /// Copy the source entry to the destination, replacing a conflicting one.
    Copy,
    /// Keep the destination's entry. The default when both platforms have one.
    Keep,
    /// Leave both platforms alone.
    Skip,
}

/// One line of a transfer preview: a source entry and, when the destination
/// already has an entry for the same media, the conflict it wins over by
/// default.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferItem {
    /// The media the entry tracks.
    pub media: MediaId,
    /// Needed to create the entry's custom lists on the destination.
    pub kind: MediaKind,
    /// The entry to copy.
    pub source: ListEntry,
    /// The destination's entry, when there is one.
    pub destination: Option<ListEntry>,
    /// The user's decision, defaulting to destination-wins on conflict.
    pub choice: TransferChoice,
}

impl TransferItem {
    /// An item with the ADR 0004 default: keep the destination on conflict,
    /// otherwise copy the source.
    #[must_use]
    pub fn new(kind: MediaKind, source: ListEntry, destination: Option<ListEntry>) -> Self {
        let choice = if destination.is_some() {
            TransferChoice::Keep
        } else {
            TransferChoice::Copy
        };
        Self {
            media: source.media,
            kind,
            source,
            destination,
            choice,
        }
    }

    /// Whether the destination already has an entry for this media.
    #[must_use]
    pub const fn is_conflict(&self) -> bool {
        self.destination.is_some()
    }
}

/// The reviewed plan shown before any entry is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPreview {
    /// Where the entries will be copied.
    pub direction: TransferDirection,
    /// The entries to review, one per media present on the source.
    pub items: Vec<TransferItem>,
    /// Custom-list names to create on the destination, grouped by media kind.
    pub custom_lists: Vec<(MediaKind, String)>,
}

impl TransferPreview {
    /// Whether there is anything to do. A transfer with no additions and no
    /// conflicts is skipped rather than shown to the user.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.custom_lists.is_empty()
    }
}

/// A progress snapshot while a transfer runs in the background.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferProgress {
    /// Which transfer is running.
    pub direction: TransferDirection,
    /// Entries reviewed so far, including those kept or skipped.
    pub done: usize,
    /// Entries in the reviewed plan.
    pub total: usize,
    /// Whether the transfer is still running or how it ended.
    pub state: TransferState,
}

/// How an in-flight transfer ended.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferState {
    /// More entries remain.
    Running,
    /// Every entry was reviewed; a re-run starts a new transfer.
    Done,
    /// The transfer stopped at an entry; a re-run resumes after the last one.
    Failed(String),
}
