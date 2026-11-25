//! List transfer behavior against fake platforms (ticket #67, ADR 0004).

#![expect(
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test harness: fake platforms and assertions index recorded data"
)]
#![expect(
    clippy::unused_async_trait_impl,
    reason = "fake platform methods finish synchronously"
)]

use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::SystemTime,
};

use domain::{
    AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score, TransferChoice,
    TransferDirection, TransferProgress, TransferState,
};
use tracker::{
    ListTransfer, PlatformError, TransferCheckpoint, TransferCheckpoints, TransferPlatform,
};

fn entry(media: i64, progress: u32, custom_lists: &[&str]) -> ListEntry {
    ListEntry {
        media: MediaId::new(media),
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
        custom_lists: custom_lists.iter().map(|name| (*name).to_owned()).collect(),
        hidden_from_status_lists: false,
        created_at: SystemTime::UNIX_EPOCH,
        updated_at: SystemTime::UNIX_EPOCH,
    }
}

#[derive(Debug, Default, Clone)]
struct Calls {
    saves: Vec<MediaId>,
    custom_lists: Vec<(MediaKind, Vec<String>)>,
}

#[derive(Clone)]
struct FakePlatform {
    entries: Arc<Mutex<HashMap<MediaKind, HashMap<MediaId, ListEntry>>>>,
    custom_lists: Arc<Mutex<HashMap<MediaKind, Vec<String>>>>,
    calls: Arc<Mutex<Calls>>,
    fail_on_save: Arc<AtomicUsize>,
}

impl FakePlatform {
    fn new(entries: Vec<ListEntry>) -> Self {
        Self {
            entries: Arc::new(Mutex::new(HashMap::from([(
                MediaKind::Anime,
                entries
                    .into_iter()
                    .map(|entry| (entry.media, entry))
                    .collect(),
            )]))),
            custom_lists: Arc::new(Mutex::new(HashMap::new())),
            calls: Arc::new(Mutex::new(Calls::default())),
            fail_on_save: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn fail_on_save(&self, nth: usize) {
        self.fail_on_save.store(nth, Ordering::SeqCst);
    }

    fn configure_custom_lists(&self, names: &[&str]) {
        self.custom_lists.lock().expect("fake custom lists").insert(
            MediaKind::Anime,
            names.iter().map(|name| (*name).to_owned()).collect(),
        );
    }

    fn calls(&self) -> Calls {
        self.calls.lock().expect("fake calls").clone()
    }

    fn saved(&self, media: i64) -> ListEntry {
        self.entries
            .lock()
            .expect("fake entries")
            .get(&MediaKind::Anime)
            .and_then(|entries| entries.get(&MediaId::new(media)))
            .cloned()
            .expect("saved entry")
    }

    fn has(&self, media: i64) -> bool {
        self.entries
            .lock()
            .expect("fake entries")
            .get(&MediaKind::Anime)
            .is_some_and(|entries| entries.contains_key(&MediaId::new(media)))
    }
}

impl TransferPlatform for FakePlatform {
    async fn entries(&self, kind: MediaKind) -> Result<Vec<ListEntry>, PlatformError> {
        let mut entries: Vec<_> = self
            .entries
            .lock()
            .expect("fake entries")
            .get(&kind)
            .into_iter()
            .flat_map(|entries| entries.values().cloned())
            .collect();
        entries.sort_by_key(|entry| entry.media);
        Ok(entries)
    }

    async fn custom_list_names(&self, kind: MediaKind) -> Result<Vec<String>, PlatformError> {
        let mut names = self
            .custom_lists
            .lock()
            .expect("fake custom lists")
            .get(&kind)
            .cloned()
            .unwrap_or_default();
        for name in self
            .entries(kind)
            .await?
            .into_iter()
            .flat_map(|entry| entry.custom_lists)
        {
            if !names.contains(&name) {
                names.push(name);
            }
        }
        names.sort();
        Ok(names)
    }

    async fn add_custom_lists(
        &self,
        kind: MediaKind,
        names: &[String],
    ) -> Result<(), PlatformError> {
        // AniList's UpdateUser replaces the whole set for a kind. The fake
        // must do the same, or a test cannot catch a transfer deleting names.
        self.custom_lists
            .lock()
            .expect("fake custom lists")
            .insert(kind, names.to_vec());
        self.calls
            .lock()
            .expect("fake calls")
            .custom_lists
            .push((kind, names.to_vec()));
        Ok(())
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        let nth = self.calls.lock().expect("fake calls").saves.len() + 1;
        let fail = self.fail_on_save.load(Ordering::SeqCst);
        if fail != 0 && nth == fail {
            return Err(PlatformError::NotConnected);
        }
        self.calls
            .lock()
            .expect("fake calls")
            .saves
            .push(entry.media);
        self.entries
            .lock()
            .expect("fake entries")
            .entry(MediaKind::Anime)
            .or_default()
            .insert(entry.media, entry.clone());
        Ok(entry.clone())
    }
}

#[derive(Clone, Default)]
struct MemoryCheckpoints(Arc<Mutex<HashMap<TransferDirection, TransferCheckpoint>>>);

impl TransferCheckpoints for MemoryCheckpoints {
    async fn load(
        &self,
        direction: TransferDirection,
    ) -> Result<Option<TransferCheckpoint>, PlatformError> {
        Ok(self
            .0
            .lock()
            .expect("fake checkpoints")
            .get(&direction)
            .cloned())
    }

    async fn save(&self, checkpoint: &TransferCheckpoint) -> Result<(), PlatformError> {
        self.0
            .lock()
            .expect("fake checkpoints")
            .insert(checkpoint.direction, checkpoint.clone());
        Ok(())
    }

    async fn clear(&self, direction: TransferDirection) -> Result<(), PlatformError> {
        self.0.lock().expect("fake checkpoints").remove(&direction);
        Ok(())
    }
}

#[tokio::test]
async fn connect_preview_adds_missing_entries_and_keeps_anilist_on_conflict() {
    let local = FakePlatform::new(vec![
        entry(1, 12, &["Watch party"]),
        entry(2, 3, &[]),
        entry(3, 8, &["Favorites"]),
    ]);
    let anilist = FakePlatform::new(vec![entry(3, 6, &[])]);
    let transfer = ListTransfer::new(
        TransferDirection::ToAniList,
        local.clone(),
        anilist.clone(),
        MemoryCheckpoints::default(),
    );

    let preview = transfer.preview().await.expect("preview");
    assert_eq!(preview.items.len(), 3, "one line per local entry");
    let conflicts: Vec<_> = preview
        .items
        .iter()
        .filter(|item| item.is_conflict())
        .collect();
    assert_eq!(conflicts.len(), 1, "only media 3 conflicts");
    assert_eq!(conflicts[0].media, MediaId::new(3));
    assert_eq!(
        conflicts[0].choice,
        TransferChoice::Keep,
        "AniList wins on connect by default"
    );
    assert!(
        preview
            .items
            .iter()
            .filter(|item| !item.is_conflict())
            .all(|item| item.choice == TransferChoice::Copy),
        "missing AniList entries are copied by default"
    );
    assert!(
        preview
            .custom_lists
            .contains(&(MediaKind::Anime, "Watch party".to_owned())),
        "custom lists are created on the destination"
    );

    let mut progress: Vec<TransferProgress> = Vec::new();
    transfer
        .apply(&preview, |update| progress.push(update))
        .await
        .expect("apply");

    assert!(anilist.has(1), "missing entry added");
    assert!(anilist.has(2), "missing entry added");
    assert_eq!(anilist.saved(3).progress, 6, "AniList's entry is untouched");
    assert!(local.has(1), "source entries are never deleted");
    assert_eq!(local.saved(3).progress, 8, "Local keeps its own entry");
    assert_eq!(
        anilist.calls().saves.len(),
        2,
        "only the two additions written"
    );
    let names = anilist
        .custom_list_names(MediaKind::Anime)
        .await
        .expect("names");
    assert_eq!(names, ["Favorites", "Watch party"]);
    assert_eq!(
        progress.last().expect("progress").state,
        TransferState::Done
    );
}

#[tokio::test]
async fn a_per_entry_override_copies_the_source_over_a_conflict() {
    let local = FakePlatform::new(vec![entry(3, 8, &[])]);
    let anilist = FakePlatform::new(vec![entry(3, 6, &[])]);
    let checkpoints = MemoryCheckpoints::default();
    let transfer = ListTransfer::new(
        TransferDirection::ToAniList,
        local,
        anilist.clone(),
        checkpoints.clone(),
    );

    let mut preview = transfer.preview().await.expect("preview");
    preview.items[0].choice = TransferChoice::Copy;
    transfer
        .apply(&preview, |_| {})
        .await
        .expect("apply override");

    assert_eq!(anilist.saved(3).progress, 8, "the override wins");
}

#[tokio::test]
async fn sign_out_keeps_local_on_conflict_by_default() {
    let anilist = FakePlatform::new(vec![entry(7, 40, &["Favorites"])]);
    let local = FakePlatform::new(vec![entry(7, 25, &[])]);
    let transfer = ListTransfer::new(
        TransferDirection::ToLocal,
        anilist.clone(),
        local.clone(),
        MemoryCheckpoints::default(),
    );

    let preview = transfer.preview().await.expect("preview");
    assert_eq!(
        preview.items[0].choice,
        TransferChoice::Keep,
        "Local wins on sign-out by default"
    );
    transfer.apply(&preview, |_| {}).await.expect("apply");
    assert_eq!(local.saved(7).progress, 25, "Local's entry is untouched");
    assert_eq!(anilist.saved(7).progress, 40, "AniList is untouched");
    assert!(local.calls().saves.is_empty(), "no entry was written");
}

#[tokio::test]
async fn an_interrupted_transfer_resumes_after_the_last_completed_entry() {
    let local = FakePlatform::new(vec![entry(1, 12, &[]), entry(2, 3, &[]), entry(3, 8, &[])]);
    let anilist = FakePlatform::new(Vec::new());
    anilist.fail_on_save(2);
    let checkpoints = MemoryCheckpoints::default();
    let transfer = ListTransfer::new(
        TransferDirection::ToAniList,
        local,
        anilist.clone(),
        checkpoints.clone(),
    );

    let preview = transfer.preview().await.expect("preview");
    let error = transfer
        .apply(&preview, |_| {})
        .await
        .expect_err("the second save fails");
    assert!(matches!(error, tracker::TransferError::Platform(_)));
    assert_eq!(anilist.calls().saves, [MediaId::new(1)]);
    let stored = checkpoints
        .load(TransferDirection::ToAniList)
        .await
        .expect("checkpoint read")
        .expect("checkpoint saved");
    assert_eq!(stored.completed.len(), 1, "one entry was confirmed done");

    anilist.fail_on_save(0);
    let mut progress: Vec<TransferProgress> = Vec::new();
    let resumed = transfer.preview().await.expect("resume preview");
    assert_eq!(resumed.items.len(), 2, "done entries are left out");
    transfer
        .apply(&resumed, |update| progress.push(update))
        .await
        .expect("resume");

    assert_eq!(anilist.calls().saves.len(), 3, "no entry is copied twice");
    assert!(anilist.has(3), "the remaining entry is copied");
    assert_eq!(progress.first().expect("progress").done, 1);
    assert_eq!(
        progress.last().expect("progress").state,
        TransferState::Done
    );
    assert!(
        checkpoints
            .load(TransferDirection::ToAniList)
            .await
            .expect("checkpoint read")
            .is_none(),
        "a finished transfer leaves no resume point"
    );
}

#[tokio::test]
async fn a_database_checkpoint_survives_a_new_transfer_instance() {
    let dir = tempfile::tempdir().expect("temp dir");
    let db = db::Db::open(&dir.path().join("autumn.db"))
        .await
        .expect("db");
    let local = FakePlatform::new(vec![entry(1, 12, &[]), entry(2, 3, &[]), entry(3, 8, &[])]);
    let anilist = FakePlatform::new(Vec::new());
    anilist.fail_on_save(2);
    let checkpoints = tracker::DbCheckpoints::new(db.clone());
    let first = ListTransfer::new(
        TransferDirection::ToAniList,
        local.clone(),
        anilist.clone(),
        checkpoints.clone(),
    );
    let preview = first.preview().await.expect("preview");
    first
        .apply(&preview, |_| {})
        .await
        .expect_err("the second save fails");
    let stored = checkpoints
        .load(TransferDirection::ToAniList)
        .await
        .expect("checkpoint read")
        .expect("checkpoint saved");
    assert_eq!(
        stored.completed,
        [MediaId::new(1)],
        "only the confirmed entry was written"
    );

    // A fresh engine over the same database is what a restarted app builds.
    let second = ListTransfer::new(
        TransferDirection::ToAniList,
        local,
        anilist.clone(),
        tracker::DbCheckpoints::new(db),
    );
    anilist.fail_on_save(0);
    let resumed = second.preview().await.expect("resume preview");
    assert_eq!(resumed.items.len(), 2, "the confirmed entry is left out");
    second.apply(&resumed, |_| {}).await.expect("resume");
    assert_eq!(
        anilist.calls().saves,
        [MediaId::new(1), MediaId::new(2), MediaId::new(3)],
        "each entry saved once, including the one from the first run"
    );
}

#[tokio::test]
async fn a_transfer_never_deletes_a_destination_only_entry() {
    let local = FakePlatform::new(vec![entry(1, 12, &["New list"])]);
    let anilist = FakePlatform::new(vec![entry(9, 99, &["Old list"])]);
    anilist.configure_custom_lists(&["Old list"]);
    let transfer = ListTransfer::new(
        TransferDirection::ToAniList,
        local,
        anilist.clone(),
        MemoryCheckpoints::default(),
    );

    let mut preview = transfer.preview().await.expect("preview");
    assert_eq!(
        preview.items.len(),
        1,
        "destination-only entries are not copied"
    );
    preview.items[0].choice = TransferChoice::Skip;
    transfer.apply(&preview, |_| {}).await.expect("apply");

    assert_eq!(anilist.saved(9).progress, 99, "the entry stays");
    assert!(anilist.calls().saves.is_empty(), "skip writes nothing");
    assert_eq!(
        anilist
            .custom_list_names(MediaKind::Anime)
            .await
            .expect("names"),
        ["New list", "Old list"],
        "an existing custom list is kept"
    );
}
