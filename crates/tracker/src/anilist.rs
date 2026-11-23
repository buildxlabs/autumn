//! The AniList tracking platform (ticket #66, ADR 0004).
//!
//! Collection reads mint local identities for AniList ids; writes go through
//! one-shot mutations, so a failed edit reaches the caller with an error the
//! UI can show alongside a Retry action. Local entries are never touched:
//! they go dormant while AniList is active and come back unchanged on sign
//! out.

use std::{
    collections::HashMap,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use db::Db;
use domain::{
    AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
};
use metadata::{AniList, ListEntryData, ListEntryInput};

use crate::{Collection, Platform, PlatformError};

/// AniList's own list-entry id, cached so delete needs no extra request.
type EntryIds = Arc<Mutex<HashMap<MediaId, i32>>>;

/// A tracking platform backed by an AniList account.
///
/// Scores always travel as AniList's raw 0–100 value (`scoreRaw` on writes,
/// `score(format: POINT_100)` on reads). The account's display format
/// therefore never needs a conversion in this platform.
pub struct AniListPlatform {
    db: Db,
    client: AniList,
    token: String,
    user_name: String,
    ids: EntryIds,
    truncated: Arc<AtomicBool>,
}

impl fmt::Debug for AniListPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never format the token: it stays in the keychain at rest (ADR 0002).
        f.debug_struct("AniListPlatform")
            .field("user_name", &self.user_name)
            .field("known_entries", &self.ids.lock().map_or(0, |ids| ids.len()))
            .finish_non_exhaustive()
    }
}

impl Clone for AniListPlatform {
    fn clone(&self) -> Self {
        Self {
            db: self.db.clone(),
            client: self.client.clone(),
            token: self.token.clone(),
            user_name: self.user_name.clone(),
            ids: Arc::clone(&self.ids),
            truncated: Arc::clone(&self.truncated),
        }
    }
}

impl AniListPlatform {
    /// Uses `token` for every authenticated request and `db` to keep the
    /// `MediaId` cross-references local.
    #[must_use]
    pub fn new(db: Db, client: AniList, token: String, user_name: String) -> Self {
        Self {
            db,
            client,
            token,
            user_name,
            ids: Arc::new(Mutex::new(HashMap::new())),
            truncated: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The account whose entries this platform reads.
    #[must_use]
    pub fn user_name(&self) -> &str {
        &self.user_name
    }

    /// Whether the last collection hit AniList's 11,000-entry cap and was
    /// therefore incomplete. The UI can show this instead of pretending the
    /// list is whole.
    #[must_use]
    pub fn collection_truncated(&self) -> bool {
        self.truncated.load(Ordering::SeqCst)
    }

    async fn anilist_id(&self, media: MediaId) -> Result<u32, PlatformError> {
        let ids = self.db.media().external_ids(media).await?;
        ids.iter()
            .find_map(|id| match id {
                ExternalId::AniList(id) => Some(*id),
                _ => None,
            })
            .ok_or(PlatformError::NoAniListId(media))
    }

    fn entry_id(&self, media: MediaId) -> Option<i32> {
        self.ids
            .lock()
            .ok()
            .and_then(|ids| ids.get(&media).copied())
    }

    fn to_entry(data: &ListEntryData, media: MediaId) -> ListEntry {
        ListEntry {
            media,
            status: data.status,
            score: Score::new(data.score_raw).unwrap_or(Score::UNSCORED),
            progress: data.progress,
            progress_volumes: data.progress_volumes,
            repeat: data.repeat,
            started_at: data.started_at,
            completed_at: data.completed_at,
            notes: data.notes.clone(),
            private: data.private,
            priority: data.priority,
            advanced_scores: AdvancedScores::default(),
            custom_lists: data.custom_lists.clone(),
            hidden_from_status_lists: data.hidden_from_status_lists,
            created_at: std::time::SystemTime::UNIX_EPOCH,
            updated_at: std::time::SystemTime::UNIX_EPOCH,
        }
    }
}

impl Platform for AniListPlatform {
    async fn collection(&self, kind: MediaKind) -> Result<Collection, PlatformError> {
        let data = self
            .client
            .list_collection(&self.token, &self.user_name, kind)
            .await?;
        // AniList caps a collection at its 11,000 most recently updated
        // entries. Collection has no room for the flag, so keep it here for
        // the UI instead of pretending the list is complete.
        self.truncated.store(data.has_next_chunk, Ordering::SeqCst);
        let mut merged: Vec<(MediaId, ListEntry)> = Vec::new();
        for group in &data.lists {
            for entry in &group.entries {
                let minted = self
                    .db
                    .media()
                    .find_or_mint(kind, &[ExternalId::AniList(entry.media_id)])
                    .await?;
                if let Some((_, existing)) =
                    merged.iter_mut().find(|(media, _)| *media == minted.media)
                {
                    for name in &entry.custom_lists {
                        if !existing.custom_lists.contains(name) {
                            existing.custom_lists.push(name.clone());
                        }
                    }
                    continue;
                }
                let list_entry = Self::to_entry(entry, minted.media);
                if let Ok(mut ids) = self.ids.lock() {
                    ids.insert(minted.media, entry.id);
                }
                merged.push((minted.media, list_entry));
            }
        }
        let entries: Vec<ListEntry> = merged.into_iter().map(|(_, entry)| entry).collect();
        Ok(Collection::group(&entries))
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        let media_id = self.anilist_id(entry.media).await?;
        let input = ListEntryInput {
            id: self.entry_id(entry.media),
            media_id: Some(media_id),
            status: Some(entry.status),
            score_raw: Some(entry.score.raw()),
            progress: Some(entry.progress),
            progress_volumes: Some(entry.progress_volumes),
            repeat: Some(entry.repeat),
            priority: Some(entry.priority),
            private: Some(entry.private),
            notes: Some(entry.notes.clone()),
            hidden_from_status_lists: Some(entry.hidden_from_status_lists),
            custom_lists: Some(entry.custom_lists.clone()),
            started_at: Some(entry.started_at),
            completed_at: Some(entry.completed_at),
        };
        let id = self.client.save_list_entry(&self.token, &input).await?;
        if let Ok(mut ids) = self.ids.lock() {
            ids.insert(entry.media, id);
        }
        Ok(entry.clone())
    }

    async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, PlatformError> {
        let media_id = self.anilist_id(media).await?;
        let input = ListEntryInput {
            id: self.entry_id(media),
            media_id: Some(media_id),
            status: Some(ListStatus::Current),
            progress: Some(progress),
            ..ListEntryInput::default()
        };
        let id = self.client.save_list_entry(&self.token, &input).await?;
        if let Ok(mut ids) = self.ids.lock() {
            ids.insert(media, id);
        }
        Ok(ListEntry {
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
            created_at: std::time::SystemTime::UNIX_EPOCH,
            updated_at: std::time::SystemTime::UNIX_EPOCH,
        })
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), PlatformError> {
        let Some(id) = self.entry_id(media) else {
            return Ok(()); // deleting an absent entry is a no-op
        };
        self.client.delete_list_entry(&self.token, id).await?;
        if let Ok(mut ids) = self.ids.lock() {
            ids.remove(&media);
        }
        Ok(())
    }
}
