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
use metadata::{AniList, CustomListNames, ListEntryData, ListEntryInput};

use crate::{Collection, Platform, PlatformError};

/// An entry as AniList last reported it, cached so targeted writes can leave
/// untouched fields alone and return the entry the UI should display.
#[derive(Debug, Clone)]
struct CachedEntry {
    id: i32,
    entry: ListEntry,
}

/// The last known entry for each local media id, keyed so delete and progress
/// updates need no collection request.
type EntryCache = Arc<Mutex<HashMap<MediaId, CachedEntry>>>;

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
    /// Every configured custom-list name, so empty lists are not lost.
    custom_names: Arc<Mutex<CustomListNames>>,
    truncated: Arc<AtomicBool>,
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
            custom_names: Arc::clone(&self.custom_names),
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
            custom_names: Arc::new(Mutex::new(CustomListNames::default())),
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

    /// Every custom-list name configured for `kind`, including lists with no
    /// entries. Needed by callers that replace the whole name set (#67).
    ///
    /// # Errors
    ///
    /// Returns the platform error when AniList cannot be read.
    pub async fn custom_list_names(&self, kind: MediaKind) -> Result<Vec<String>, PlatformError> {
        self.collection(kind).await?;
        Ok(self.custom_names.lock().map_or_else(
            |_| Vec::new(),
            |names| match kind {
                MediaKind::Anime => names.anime.clone(),
                MediaKind::Manga => names.manga.clone(),
                _ => Vec::new(),
            },
        ))
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
            .and_then(|entries| entries.get(&media).map(|cached| cached.id))
    }

    fn cached_entry(&self, media: MediaId) -> Option<ListEntry> {
        self.entries
            .lock()
            .ok()
            .and_then(|entries| entries.get(&media).map(|cached| cached.entry.clone()))
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
        self.truncated
            .fetch_or(data.has_next_chunk, Ordering::SeqCst);
        if let Ok(mut names) = self.custom_names.lock() {
            *names = data.custom_lists.clone();
        }
        let custom_names: Vec<String> = match kind {
            MediaKind::Anime => data.custom_lists.anime.clone(),
            MediaKind::Manga => data.custom_lists.manga.clone(),
            _ => Vec::new(),
        };
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
        let entries: Vec<ListEntry> = order
            .iter()
            .filter_map(|media| merged.get(media).map(|(_, entry)| entry.clone()))
            .collect();
        if let Ok(mut cache) = self.entries.lock() {
            for media in order {
                if let Some((id, entry)) = merged.remove(&media) {
                    cache.insert(media, CachedEntry { id, entry });
                }
            }
        }
        Ok(Collection::group(&entries, &custom_names))
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
        if let Ok(mut cache) = self.entries.lock() {
            cache.insert(
                entry.media,
                CachedEntry {
                    id,
                    entry: entry.clone(),
                },
            );
        }
        Ok(entry.clone())
    }

    async fn update_progress(
        &self,
        media: MediaId,
        progress: u32,
    ) -> Result<ListEntry, PlatformError> {
        let mut cached = self.cached_entry(media);
        if cached.is_none() {
            // Cold start: an entry already on AniList must keep its status.
            // Look it up the same way delete does before deciding it is new.
            self.collection(MediaKind::Anime).await?;
            if self.cached_entry(media).is_none() {
                self.collection(MediaKind::Manga).await?;
            }
            cached = self.cached_entry(media);
        }
        let input = ListEntryInput {
            id: self.entry_id(media),
            media_id: Some(self.anilist_id(media).await?),
            // Only a brand-new progress update implies watching; an existing
            // entry keeps whatever status the user set.
            status: cached.is_none().then_some(ListStatus::Current),
            progress: Some(progress),
            ..ListEntryInput::default()
        };
        let id = self.client.save_list_entry(&self.token, &input).await?;
        let entry = match cached {
            Some(mut entry) => {
                entry.progress = progress;
                entry
            }
            None => ListEntry {
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
            },
        };
        if let Ok(mut cache) = self.entries.lock() {
            cache.insert(
                media,
                CachedEntry {
                    id,
                    entry: entry.clone(),
                },
            );
        }
        Ok(entry)
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), PlatformError> {
        let id = if let Some(id) = self.entry_id(media) {
            id
        } else {
            // Cold start, or an entry past the 11k cap: find its list entry
            // id before deciding the entry is absent. A wrong "no-op" here
            // would leave the entry undeletable.
            self.collection(MediaKind::Anime).await?;
            if self.entry_id(media).is_none() {
                self.collection(MediaKind::Manga).await?;
            }
            let Some(id) = self.entry_id(media) else {
                return Ok(());
            };
            id
        };
        self.client.delete_list_entry(&self.token, id).await?;
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(&media);
        }
        Ok(())
    }
}
