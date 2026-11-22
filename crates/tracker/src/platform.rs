use std::future::Future;

use domain::{ListEntry, MediaId, MediaKind};

use crate::{Collection, TrackerError};

/// Where a user's list entries live: the account-free [`Local`](crate::Local)
/// platform, or a tracking service account such as AniList.
///
/// Every method acts on this platform only; nothing is mirrored to another
/// platform (ADR 0004). The trait is not object-safe: the engine holds its
/// one active platform as a concrete type.
pub trait Platform {
    /// The user's whole list for `kind` media, grouped by status and custom
    /// list.
    ///
    /// # Errors
    ///
    /// [`TrackerError`] when the list can't be read.
    fn collection(
        &self,
        kind: MediaKind,
    ) -> impl Future<Output = Result<Collection, TrackerError>> + Send;

    /// Adds `entry` to the list, or replaces the entry for its media, and
    /// returns it as stored. The platform sets `created_at` and `updated_at`;
    /// the values in `entry` are ignored.
    ///
    /// # Errors
    ///
    /// [`TrackerError::UnknownMedia`], or [`TrackerError`] when the entry
    /// can't be saved.
    fn save_entry(
        &self,
        entry: &ListEntry,
    ) -> impl Future<Output = Result<ListEntry, TrackerError>> + Send;

    /// Sets the episodes watched or chapters read for `media` and returns
    /// the entry. A media not in the list is added as
    /// [`Current`](domain::ListStatus::Current), as playback progress implies.
    ///
    /// # Errors
    ///
    /// [`TrackerError::UnknownMedia`], or [`TrackerError`] when the entry
    /// can't be saved.
    fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> impl Future<Output = Result<ListEntry, TrackerError>> + Send;

    /// Removes the entry for `media` from the list. Removing a media that is
    /// not in the list does nothing.
    ///
    /// # Errors
    ///
    /// [`TrackerError`] when the entry can't be removed.
    fn delete_entry(&self, media: MediaId)
    -> impl Future<Output = Result<(), TrackerError>> + Send;
}
