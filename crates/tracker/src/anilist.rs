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
use domain::{AdvancedScores, ExternalId, ListEntry, ListStatus, MediaId, MediaKind, Score};
use metadata::{AniList, CustomListNames, ListEntryData, ListEntryInput};

use crate::{Collection, Platform, PlatformError};

/// The AniList list-entry id for each local media id, so delete and progress
/// updates need no lookup request. Ids learned from a collection carry its
/// kind; ids learned from a write or a single-entry lookup carry `None`.
type EntryCache = Arc<Mutex<HashMap<MediaId, (Option<MediaKind>, i32)>>>;

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
    entries: EntryCache,
    truncated: Arc<Truncated>,
}

/// Whether the latest anime and manga collections hit the 11k cap.
#[derive(Debug, Default)]
struct Truncated {
    anime: AtomicBool,
    manga: AtomicBool,
}

impl Truncated {
    fn flag(&self, kind: MediaKind) -> Option<&AtomicBool> {
        match kind {
            MediaKind::Anime => Some(&self.anime),
            MediaKind::Manga => Some(&self.manga),
            _ => None,
        }
    }
}

impl fmt::Debug for AniListPlatform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Never format the token: it stays in the keychain at rest (ADR 0002).
        f.debug_struct("AniListPlatform")
            .field("user_name", &self.user_name)
            .field(
                "known_entries",
                &self.entries.lock().map_or(0, |entries| entries.len()),
            )
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
            entries: Arc::clone(&self.entries),
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
            entries: Arc::new(Mutex::new(HashMap::new())),
            truncated: Arc::new(Truncated::default()),
        }
    }

    /// The account whose entries this platform reads.
    #[must_use]
    pub fn user_name(&self) -> &str {
        &self.user_name
    }

    /// Whether the latest `kind` collection hit AniList's 11,000-entry cap
    /// and was therefore incomplete. The UI can show this instead of
    /// pretending the list is whole.
    #[must_use]
    pub fn collection_truncated(&self, kind: MediaKind) -> bool {
        self.truncated
            .flag(kind)
            .is_some_and(|flag| flag.load(Ordering::SeqCst))
    }

    /// Every custom-list name configured on the account, for both kinds,
    /// including lists with no entries. Read fresh from AniList's list
    /// options in one small request, without downloading the collection.
    ///
    /// A caller that replaces the name set (the list transfer in #67) needs
    /// the full current set: `UpdateUser` deletes every name it is not given.
    ///
    /// # Errors
    ///
    /// Returns the platform error when AniList cannot be read.
    pub async fn custom_list_names(&self) -> Result<CustomListNames, PlatformError> {
        Ok(self.client.custom_list_names(&self.token).await?)
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
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&media).map(|&(_, id)| id))
    }

    /// The media's AniList list-entry id: cached, or looked up with one
    /// small request. `None` means the media is not on the account's list.
    /// The lookup also finds entries past the collection's 11k cap.
    async fn find_entry_id(&self, media: MediaId) -> Result<Option<i32>, PlatformError> {
        if let Some(id) = self.entry_id(media) {
            return Ok(Some(id));
        }
        let anilist_id = self.anilist_id(media).await?;
        let Some(found) = self
            .client
            .list_entry(&self.token, &self.user_name, anilist_id)
            .await?
        else {
            return Ok(None);
        };
        if let Ok(mut cache) = self.entries.lock() {
            cache.insert(media, (None, found.id));
        }
        Ok(Some(found.id))
    }

    /// Caches the id of the entry AniList returned from a write and converts it for
    /// the caller, so the UI shows what was stored rather than what was sent.
    fn remember(&self, media: MediaId, saved: &ListEntryData) -> ListEntry {
        if let Ok(mut cache) = self.entries.lock() {
            cache.insert(media, (None, saved.id));
        }
        Self::to_entry(saved, media)
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
        if let Some(flag) = self.truncated.flag(kind) {
            flag.store(data.has_next_chunk, Ordering::SeqCst);
        }
        let custom_names = data.custom_lists.for_kind(kind);
        let mut merged: HashMap<MediaId, (i32, ListEntry)> = HashMap::new();
        let mut order: Vec<MediaId> = Vec::new();
        for group in &data.lists {
            for entry in &group.entries {
                let minted = self
                    .db
                    .media()
                    .find_or_mint(kind, &[ExternalId::AniList(entry.media_id)])
                    .await?;
                if let Some((_, existing)) = merged.get_mut(&minted.media) {
                    for name in &entry.custom_lists {
                        if !existing.custom_lists.contains(name) {
                            existing.custom_lists.push(name.clone());
                        }
                    }
                    continue;
                }
                merged.insert(
                    minted.media,
                    (entry.id, Self::to_entry(entry, minted.media)),
                );
                order.push(minted.media);
            }
        }
        let mut entries = Vec::with_capacity(order.len());
        let mut ids = Vec::with_capacity(order.len());
        for media in order {
            if let Some((id, entry)) = merged.remove(&media) {
                ids.push((media, (Some(kind), id)));
                entries.push(entry);
            }
        }
        if let Ok(mut cache) = self.entries.lock() {
            // Forget ids this fetch could have confirmed: an entry deleted on
            // anilist.co must not keep a stale id that makes every save fail.
            // Untagged ids are cheap to look up again, so they go too.
            cache.retain(|_, (cached, _)| cached.is_some_and(|cached| cached != kind));
            cache.extend(ids);
        }
        Ok(Collection::group(&entries, custom_names))
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
        let saved = self.client.save_list_entry(&self.token, &input).await?;
        Ok(self.remember(entry.media, &saved))
    }

    async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, PlatformError> {
        // Cold start: an entry already on AniList must keep its status, so
        // look it up before deciding it is new.
        let id = self.find_entry_id(media).await?;
        let input = ListEntryInput {
            id,
            media_id: Some(self.anilist_id(media).await?),
            // Only a brand-new progress update implies watching; an existing
            // entry keeps whatever status the user set.
            status: id.is_none().then_some(ListStatus::Current),
            progress: Some(progress),
            ..ListEntryInput::default()
        };
        let saved = self.client.save_list_entry(&self.token, &input).await?;
        Ok(self.remember(media, &saved))
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), PlatformError> {
        // Cold start, or an entry past the 11k cap: find its list-entry id
        // before deciding the entry is absent.
        let Some(id) = self.find_entry_id(media).await? else {
            return Ok(());
        };
        self.client.delete_list_entry(&self.token, id).await?;
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&media);
        }
        Ok(())
    }
}
