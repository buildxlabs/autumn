use std::future::Future;

use domain::{ListEntry, MediaId, MediaKind};

use crate::{Collection, PlatformError};

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
    /// [`PlatformError`] when the list can't be read.
    fn collection(
        &self,
        kind: MediaKind,
    ) -> impl Future<Output = Result<Collection, PlatformError>> + Send;

    /// Adds `entry` to the list, or replaces the entry for its media, and
    /// returns it as stored. The platform sets `created_at` and `updated_at`;
    /// the values in `entry` are ignored.
    ///
    /// # Errors
    ///
    /// [`PlatformError::UnknownMedia`], or [`PlatformError`] when the entry
    /// can't be saved.
    fn save_entry(
        &self,
        entry: &ListEntry,
    ) -> impl Future<Output = Result<ListEntry, PlatformError>> + Send;

    /// Sets the episodes watched or chapters read for `media` and returns
    /// the entry. A media not in the list is added as
    /// [`Current`](domain::ListStatus::Current), as playback progress implies.
    ///
    /// # Errors
    ///
    /// [`PlatformError::UnknownMedia`], or [`PlatformError`] when the entry
    /// can't be saved.
    fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> impl Future<Output = Result<ListEntry, PlatformError>> + Send;

    /// Removes the entry for `media` from the list. Removing a media that is
    /// not in the list does nothing.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the entry can't be removed.
    fn delete_entry(
        &self,
        media: MediaId,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Creates any custom-list names on this platform that are missing.
    ///
    /// The default does nothing: the Local platform stores names on entries.
    /// AniList replaces all names for a kind, so callers pass the union of
    /// existing and new names; a subset would delete the others.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the names cannot be stored.
    fn ensure_custom_lists(
        &self,
        _kind: MediaKind,
        _names: &[String],
    ) -> impl Future<Output = Result<(), PlatformError>> + Send {
        async { Ok(()) }
    }
}
