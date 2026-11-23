//! User-reviewed list transfer between platforms (ADR 0004).
//!
//! Planning is pure: it reads both collections and proposes an action per
//! source list entry. Running writes only to the destination. It never calls
//! `delete_entry`, and an interrupted run can simply be planned and run again:
//! entries already copied are now conflicts, whose default action is to keep
//! the destination.

use std::collections::{HashMap, HashSet};

use domain::{
    ListEntry, MediaId, MediaKind, TransferAction, TransferDirection, TransferEntry,
    TransferPreview, TransferProgress,
};

use crate::{Collection, Platform, PlatformError};

/// The distinct list entries in a collection, in list order.
#[must_use]
pub fn collection_entries(collection: &Collection) -> Vec<ListEntry> {
    let mut seen = HashSet::new();
    collection
        .lists
        .iter()
        .flat_map(|list| list.entries.iter())
        .filter(|entry| seen.insert(entry.media))
        .cloned()
        .collect()
}

/// Builds the preview for `kind`: every source list entry, with a conflict
/// when the destination already has that media.
///
/// Conflicts default to [`TransferAction::Keep`]; the UI may change individual
/// lines to [`TransferAction::Replace`] before the run. `custom_lists` is the
/// sorted union of names on both platforms, so creating them on the
/// destination cannot delete an existing name.
#[must_use]
pub fn plan_transfer(
    kind: MediaKind,
    direction: TransferDirection,
    source: &[ListEntry],
    destination: &[ListEntry],
) -> TransferPreview {
    let by_media: HashMap<MediaId, &ListEntry> = destination
        .iter()
        .map(|entry| (entry.media, entry))
        .collect();
    let mut names: Vec<String> = source
        .iter()
        .chain(destination.iter())
        .flat_map(|entry| entry.custom_lists.iter().cloned())
        .collect();
    names.sort();
    names.dedup();
    let entries = source
        .iter()
        .map(|source| {
            let destination = by_media.get(&source.media).map(|entry| (*entry).clone());
            let action = if destination.is_some() {
                TransferAction::Keep
            } else {
                TransferAction::Add
            };
            TransferEntry {
                source: source.clone(),
                destination,
                action,
            }
        })
        .collect();
    TransferPreview {
        direction,
        kind,
        entries,
        custom_lists: names,
    }
}

/// Writes the preview's pending lines to `destination`, reporting progress
/// after each one. Nothing is deleted from either platform.
///
/// # Errors
///
/// [`PlatformError`] when a custom-list name or a list entry cannot be saved.
/// A finished entry is already durable; re-planning and re-running skips it.
pub async fn run_transfer<P: Platform>(
    preview: &TransferPreview,
    destination: &P,
    mut progress: impl FnMut(TransferProgress),
) -> Result<(), PlatformError> {
    let pending = preview.pending();
    if pending.is_empty() {
        return Ok(());
    }
    if !preview.custom_lists.is_empty()
        && pending
            .iter()
            .any(|line| !line.source.custom_lists.is_empty())
    {
        destination
            .ensure_custom_lists(preview.kind, &preview.custom_lists)
            .await?;
    }
    let total = pending.len();
    for (index, line) in pending.iter().enumerate() {
        destination.save_entry(&line.source).await?;
        progress(TransferProgress {
            done: index + 1,
            total,
        });
    }
    Ok(())
}
