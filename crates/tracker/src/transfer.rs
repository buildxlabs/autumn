//! Reviewed list transfers between the Local and AniList platforms (#67,
//! ADR 0004). The engine writes only to the destination; the trait bounds have
//! no delete method, so a transfer cannot remove an entry even by mistake.
//!
//! An interrupted transfer leaves a checkpoint in the database. Re-running it
//! previews only the entries that were not confirmed, and resumes with the
//! choices the user reviewed the first time.

use std::{
    collections::{BTreeMap, BTreeSet, HashMap},
    future::Future,
};

use db::Db;
use domain::{
    ListEntry, MediaId, MediaKind, TransferChoice, TransferDirection, TransferItem,
    TransferPreview, TransferProgress, TransferState,
};
use serde::{Deserialize, Serialize};

use crate::{AniListPlatform, Collection, Local, Platform, PlatformError};

/// Why a list transfer could not finish.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TransferError {
    /// A platform read, write or checkpoint failed.
    #[error("the list transfer could not continue")]
    Platform(#[from] PlatformError),
}

/// The part of a platform a transfer needs. It deliberately has no delete
/// method: a transfer creates or updates entries, never removes them.
pub trait TransferPlatform {
    /// The platform's list entries, deduplicated across status and custom
    /// lists.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the collection cannot be read.
    fn entries(
        &self,
        kind: MediaKind,
    ) -> impl Future<Output = Result<Vec<ListEntry>, PlatformError>> + Send;

    /// Every custom-list name configured for `kind`, including empty lists.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the names cannot be read.
    fn custom_list_names(
        &self,
        kind: MediaKind,
    ) -> impl Future<Output = Result<Vec<String>, PlatformError>> + Send;

    /// Replaces `kind`'s configured custom lists with `names`. AniList's
    /// `UpdateUser` works this way, so the engine passes every existing name
    /// plus the new ones: nothing is deleted. Called before entries that use
    /// the names are written.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the names cannot be saved.
    fn add_custom_lists(
        &self,
        kind: MediaKind,
        names: &[String],
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Creates or replaces one list entry.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the entry cannot be saved.
    fn save_entry(
        &self,
        entry: &ListEntry,
    ) -> impl Future<Output = Result<ListEntry, PlatformError>> + Send;
}

impl TransferPlatform for Local {
    async fn entries(&self, kind: MediaKind) -> Result<Vec<ListEntry>, PlatformError> {
        Ok(deduplicated(Platform::collection(self, kind).await?))
    }

    async fn custom_list_names(&self, kind: MediaKind) -> Result<Vec<String>, PlatformError> {
        let names = self
            .entries(kind)
            .await?
            .into_iter()
            .flat_map(|entry| entry.custom_lists)
            .collect::<BTreeSet<_>>();
        Ok(names.into_iter().collect())
    }

    fn add_custom_lists(
        &self,
        _kind: MediaKind,
        _names: &[String],
    ) -> impl Future<Output = Result<(), PlatformError>> + Send {
        // Local has no configured lists: the names travel on each entry.
        std::future::ready(Ok(()))
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        Platform::save_entry(self, entry).await
    }
}

impl TransferPlatform for AniListPlatform {
    async fn entries(&self, kind: MediaKind) -> Result<Vec<ListEntry>, PlatformError> {
        Ok(deduplicated(Platform::collection(self, kind).await?))
    }

    async fn custom_list_names(&self, kind: MediaKind) -> Result<Vec<String>, PlatformError> {
        Ok(self.custom_list_names().await?.for_kind(kind).to_vec())
    }

    async fn add_custom_lists(
        &self,
        kind: MediaKind,
        names: &[String],
    ) -> Result<(), PlatformError> {
        AniListPlatform::add_custom_lists(self, kind, names).await
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        Platform::save_entry(self, entry).await
    }
}

/// Where a transfer keeps its reviewed choices and confirmed entries.
pub trait TransferCheckpoints {
    /// The unfinished transfer for `direction`, if any.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the checkpoint cannot be read or decoded.
    fn load(
        &self,
        direction: TransferDirection,
    ) -> impl Future<Output = Result<Option<TransferCheckpoint>, PlatformError>> + Send;

    /// Replaces the stored checkpoint.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the checkpoint cannot be encoded or saved.
    fn save(
        &self,
        checkpoint: &TransferCheckpoint,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;

    /// Removes the checkpoint once the transfer is finished.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when the delete fails.
    fn clear(
        &self,
        direction: TransferDirection,
    ) -> impl Future<Output = Result<(), PlatformError>> + Send;
}

/// A transfer's reviewed choices and the entries already written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TransferCheckpoint {
    /// Which transfer this belongs to.
    pub direction: TransferDirection,
    /// The reviewed decision for each entry.
    pub items: Vec<CheckpointItem>,
    /// Entries confirmed on the destination, so a re-run skips them.
    pub completed: Vec<MediaId>,
}

/// One reviewed decision in a [`TransferCheckpoint`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointItem {
    /// The media the entry tracks.
    pub media: MediaId,
    /// Needed when the destination creates the entry's custom lists.
    pub kind: MediaKind,
    /// The decision the user reviewed.
    pub choice: TransferChoice,
}

/// Checkpoints stored in the app database, one row per direction.
#[derive(Debug, Clone)]
pub struct DbCheckpoints {
    db: Db,
}

impl DbCheckpoints {
    /// Stores resume points in `db`.
    #[must_use]
    pub const fn new(db: Db) -> Self {
        Self { db }
    }
}

impl TransferCheckpoints for DbCheckpoints {
    async fn load(
        &self,
        direction: TransferDirection,
    ) -> Result<Option<TransferCheckpoint>, PlatformError> {
        let Some(json) = self.db.transfer().load(key(direction)).await? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_str(&json)?))
    }

    async fn save(&self, checkpoint: &TransferCheckpoint) -> Result<(), PlatformError> {
        let json = serde_json::to_string(checkpoint)?;
        self.db
            .transfer()
            .save(key(checkpoint.direction), &json)
            .await?;
        Ok(())
    }

    async fn clear(&self, direction: TransferDirection) -> Result<(), PlatformError> {
        self.db.transfer().clear(key(direction)).await?;
        Ok(())
    }
}

const fn key(direction: TransferDirection) -> &'static str {
    match direction {
        TransferDirection::ToAniList => "connect",
        TransferDirection::ToLocal => "sign_out",
    }
}

/// How many entries a transfer wrote, kept or skipped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TransferOutcome {
    /// Entries copied to the destination.
    pub applied: usize,
    /// Conflicts where the destination was kept.
    pub kept: usize,
    /// Entries the user chose to leave alone.
    pub skipped: usize,
}

/// A one-shot copy of list entries from `source` to `destination`.
#[derive(Debug, Clone)]
pub struct ListTransfer<S, D, C> {
    direction: TransferDirection,
    source: S,
    destination: D,
    checkpoints: C,
}

impl<S, D, C> ListTransfer<S, D, C>
where
    S: TransferPlatform,
    D: TransferPlatform,
    C: TransferCheckpoints,
{
    /// A transfer in `direction`, using `checkpoints` to resume.
    #[must_use]
    pub const fn new(
        direction: TransferDirection,
        source: S,
        destination: D,
        checkpoints: C,
    ) -> Self {
        Self {
            direction,
            source,
            destination,
            checkpoints,
        }
    }

    /// Builds the plan for the user to review. Identical entries and entries
    /// already confirmed by an earlier run are left out; decisions from an
    /// unfinished run are kept.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when either platform or the checkpoint cannot be read.
    pub async fn preview(&self) -> Result<TransferPreview, TransferError> {
        let saved = self.checkpoints.load(self.direction).await?;
        let mut items = Vec::new();
        let mut custom_lists = BTreeSet::new();
        for kind in [MediaKind::Anime, MediaKind::Manga] {
            let source = self.source.entries(kind).await?;
            let configured = self.destination.custom_list_names(kind).await?;
            let mut destination: HashMap<MediaId, ListEntry> = self
                .destination
                .entries(kind)
                .await?
                .into_iter()
                .map(|entry| (entry.media, entry))
                .collect();
            for entry in source {
                for name in &entry.custom_lists {
                    if !configured.contains(name) {
                        custom_lists.insert((kind, name.clone()));
                    }
                }
                let other = destination.remove(&entry.media);
                if other
                    .as_ref()
                    .is_some_and(|other| same_entry(&entry, other))
                {
                    continue;
                }
                if saved
                    .as_ref()
                    .is_some_and(|saved| saved.completed.contains(&entry.media))
                {
                    continue;
                }
                let mut item = TransferItem::new(kind, entry, other);
                if let Some(choice) = saved.as_ref().and_then(|saved| {
                    saved
                        .items
                        .iter()
                        .find(|saved| saved.media == item.media)
                        .map(|saved| saved.choice)
                }) {
                    item.choice = choice;
                }
                items.push(item);
            }
        }
        items.sort_by_key(|item| (item.kind, item.media));
        Ok(TransferPreview {
            direction: self.direction,
            items,
            custom_lists: custom_lists.into_iter().collect(),
        })
    }

    /// Applies the reviewed plan, reporting progress after every entry.
    /// Custom lists are created first so entries can refer to them. Each
    /// confirmed entry is checkpointed before the next one starts, so an
    /// interruption can be resumed without repeating a write.
    ///
    /// # Errors
    ///
    /// [`PlatformError`] when a write or a checkpoint fails. The checkpoint
    /// left on disk keeps every entry confirmed before that point.
    pub async fn apply(
        &self,
        preview: &TransferPreview,
        mut progress: impl FnMut(TransferProgress),
    ) -> Result<TransferOutcome, TransferError> {
        let mut checkpoint = self
            .checkpoints
            .load(self.direction)
            .await?
            .unwrap_or_else(|| TransferCheckpoint {
                direction: self.direction,
                items: Vec::new(),
                completed: Vec::new(),
            });
        for item in &preview.items {
            let reviewed = CheckpointItem {
                media: item.media,
                kind: item.kind,
                choice: item.choice,
            };
            if let Some(saved) = checkpoint
                .items
                .iter_mut()
                .find(|saved| saved.media == item.media)
            {
                *saved = reviewed;
            } else {
                checkpoint.items.push(reviewed);
            }
        }
        let mut by_kind: BTreeMap<MediaKind, Vec<String>> = BTreeMap::new();
        for (kind, name) in &preview.custom_lists {
            by_kind.entry(*kind).or_default().push(name.clone());
        }
        for (kind, names) in by_kind {
            let mut desired = self.destination.custom_list_names(kind).await?;
            let mut changed = false;
            for name in names {
                if !desired.contains(&name) {
                    desired.push(name);
                    changed = true;
                }
            }
            if changed {
                self.destination.add_custom_lists(kind, &desired).await?;
            }
        }
        self.checkpoints.save(&checkpoint).await?;

        let remaining: Vec<_> = preview
            .items
            .iter()
            .filter(|item| !checkpoint.completed.contains(&item.media))
            .collect();
        let total = remaining.len();
        let mut outcome = TransferOutcome::default();
        for (index, item) in remaining.iter().enumerate() {
            match item.choice {
                TransferChoice::Copy => {
                    self.destination.save_entry(&item.source).await?;
                    outcome.applied += 1;
                }
                TransferChoice::Keep => outcome.kept += 1,
                TransferChoice::Skip => outcome.skipped += 1,
                _ => {}
            }
            checkpoint.completed.push(item.media);
            self.checkpoints.save(&checkpoint).await?;
            progress(TransferProgress {
                direction: self.direction,
                done: index + 1,
                total,
                state: TransferState::Running,
            });
        }
        self.checkpoints.clear(self.direction).await?;
        progress(TransferProgress {
            direction: self.direction,
            done: total,
            total,
            state: TransferState::Done,
        });
        Ok(outcome)
    }
}

fn deduplicated(collection: Collection) -> Vec<ListEntry> {
    let mut seen = std::collections::HashSet::new();
    collection
        .lists
        .into_iter()
        .flat_map(|list| list.entries)
        .filter(|entry| seen.insert(entry.media))
        .collect()
}

/// Whether two entries hold the same transferable fields. Timestamps are
/// owned by the destination. AniList's Phase 1 platform cannot write advanced
/// scores, so a Local entry that has them is shown as a conflict rather than
/// quietly treated as already transferred.
fn same_entry(left: &ListEntry, right: &ListEntry) -> bool {
    left.status == right.status
        && left.score == right.score
        && left.advanced_scores == right.advanced_scores
        && left.progress == right.progress
        && left.progress_volumes == right.progress_volumes
        && left.repeat == right.repeat
        && left.started_at == right.started_at
        && left.completed_at == right.completed_at
        && left.notes == right.notes
        && left.private == right.private
        && left.priority == right.priority
        && left.custom_lists == right.custom_lists
        && left.hidden_from_status_lists == right.hidden_from_status_lists
}
