//! Plain types for a user-reviewed list transfer (ADR 0004).
//!
//! A list transfer is a one-shot copy. It creates or updates list entries and
//! never deletes them. These types travel from the engine through the event
//! pump to the preview dialog; no I/O lives here.

use crate::{ListEntry, MediaId, MediaKind};

/// Which way a list transfer runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferDirection {
    /// Local entries are copied to AniList, offered on connect.
    LocalToAniList,
    /// AniList entries are copied to Local, offered on sign-out.
    AniListToLocal,
}

/// What the runner does with one source list entry.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum TransferAction {
    /// Copy the source entry because the destination has none.
    #[default]
    Add,
    /// Keep the destination's entry. The default for a conflict.
    Keep,
    /// Replace the destination's entry with the source's, an explicit
    /// per-entry override of the conflict default.
    Replace,
}

/// One line of the transfer preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferEntry {
    /// The entry that would be copied.
    pub source: ListEntry,
    /// The entry already on the destination, if there is a conflict.
    pub destination: Option<ListEntry>,
    /// What the runner will do when the preview is confirmed.
    pub action: TransferAction,
}

impl TransferEntry {
    /// The media this line transfers.
    #[must_use]
    pub fn media(&self) -> MediaId {
        self.source.media
    }

    /// Whether the destination already has an entry for this media.
    #[must_use]
    pub fn is_conflict(&self) -> bool {
        self.destination.is_some()
    }
}

/// The reviewed plan shown to the user before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransferPreview {
    /// Which platform receives the entries.
    pub direction: TransferDirection,
    /// The media kind the planner read.
    pub kind: MediaKind,
    /// Every source entry, with its default action.
    pub entries: Vec<TransferEntry>,
    /// Union of custom-list names on both platforms, to create on the
    /// destination before any entry references them.
    pub custom_lists: Vec<String>,
}

impl TransferPreview {
    /// Lines the runner will write on confirmation.
    #[must_use]
    pub fn pending(&self) -> Vec<&TransferEntry> {
        self.entries
            .iter()
            .filter(|entry| entry.action != TransferAction::Keep)
            .collect()
    }

    /// Lines where both platforms have an entry.
    #[must_use]
    pub fn conflicts(&self) -> usize {
        self.entries
            .iter()
            .filter(|entry| entry.is_conflict())
            .count()
    }
}

/// How far a background transfer has got.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferProgress {
    /// Entries written so far.
    pub done: usize,
    /// Entries the runner will write in total.
    pub total: usize,
}
