//! Fake-platform tests for the reviewed list transfer (ticket #67, ADR 0004).

#![expect(
    clippy::indexing_slicing,
    clippy::unused_async_trait_impl,
    clippy::unwrap_used,
    reason = "fake platforms mirror the async Platform trait and index recorded state"
)]

use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

use domain::{
    AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score, TransferAction,
    TransferDirection, TransferPreview, TransferProgress,
};
use tracker::{
    Collection, List, ListName, Platform, PlatformError, collection_entries, plan_transfer,
    run_transfer,
};

#[derive(Debug, Default)]
struct State {
    entries: HashMap<MediaId, ListEntry>,
    custom_lists: Vec<String>,
    saves: usize,
    deletes: usize,
    fail_on_save: Option<usize>,
}

#[derive(Debug, Clone, Default)]
struct FakePlatform {
    state: Arc<Mutex<State>>,
}

impl FakePlatform {
    fn with(entries: Vec<ListEntry>) -> Self {
        let state = State {
            entries: entries
                .into_iter()
                .map(|entry| (entry.media, entry))
                .collect(),
            ..State::default()
        };
        Self {
            state: Arc::new(Mutex::new(state)),
        }
    }

    fn snapshot(&self) -> State {
        let state = self.state.lock().unwrap();
        State {
            entries: state.entries.clone(),
            custom_lists: state.custom_lists.clone(),
            saves: state.saves,
            deletes: state.deletes,
            fail_on_save: state.fail_on_save,
        }
    }

    fn collection(&self) -> Collection {
        let state = self.state.lock().unwrap();
        let mut entries: Vec<ListEntry> = state.entries.values().cloned().collect();
        entries.sort_by_key(|entry| entry.media.get());
        Collection {
            lists: vec![List {
                name: ListName::Status(ListStatus::Current),
                entries,
            }],
        }
    }
}

impl Platform for FakePlatform {
    async fn collection(&self, _kind: MediaKind) -> Result<Collection, PlatformError> {
        Ok(self.collection())
    }

    async fn save_entry(&self, entry: &ListEntry) -> Result<ListEntry, PlatformError> {
        let mut state = self.state.lock().unwrap();
        state.saves += 1;
        if state.fail_on_save == Some(state.saves) {
            return Err(PlatformError::NotConnected);
        }
        state.entries.insert(entry.media, entry.clone());
        Ok(entry.clone())
    }

    async fn update_progress(
        &self,
        _media: MediaId,
        _progress: u32,
    ) -> Result<ListEntry, PlatformError> {
        Err(PlatformError::NotConnected)
    }

    async fn delete_entry(&self, media: MediaId) -> Result<(), PlatformError> {
        let mut state = self.state.lock().unwrap();
        state.deletes += 1;
        state.entries.remove(&media);
        Ok(())
    }

    async fn ensure_custom_lists(
        &self,
        _kind: MediaKind,
        names: &[String],
    ) -> Result<(), PlatformError> {
        let mut state = self.state.lock().unwrap();
        state.custom_lists = names.to_vec();
        Ok(())
    }
}

fn media(id: i64) -> MediaId {
    MediaId::new(id)
}

fn entry(id: i64, notes: &str, custom_lists: &[&str]) -> ListEntry {
    ListEntry {
        media: media(id),
        status: ListStatus::Current,
        score: Score::UNSCORED,
        progress: 1,
        progress_volumes: 0,
        repeat: 0,
        started_at: FuzzyDate::UNKNOWN,
        completed_at: FuzzyDate::UNKNOWN,
        notes: notes.to_owned(),
        private: false,
        priority: 0,
        advanced_scores: AdvancedScores::default(),
        custom_lists: custom_lists.iter().map(|name| (*name).to_owned()).collect(),
        hidden_from_status_lists: false,
        created_at: std::time::SystemTime::UNIX_EPOCH,
        updated_at: std::time::SystemTime::UNIX_EPOCH,
    }
}

fn plan(source: &FakePlatform, destination: &FakePlatform) -> TransferPreview {
    plan_transfer(
        MediaKind::Anime,
        TransferDirection::LocalToAniList,
        &collection_entries(&source.collection()),
        &collection_entries(&destination.collection()),
    )
}

#[tokio::test]
async fn conflict_keeps_destination_unless_overridden() {
    let source = FakePlatform::with(vec![
        entry(1, "new a", &["Favorites"]),
        entry(2, "new b", &[]),
        entry(3, "new c", &[]),
    ]);
    let destination = FakePlatform::with(vec![entry(2, "kept", &["Existing"])]);

    let preview = plan(&source, &destination);
    assert_eq!(preview.conflicts(), 1, "only media 2 already exists");
    let actions: Vec<_> = preview
        .entries
        .iter()
        .map(|line| (line.media().get(), line.action))
        .collect();
    assert_eq!(
        actions,
        vec![
            (1, TransferAction::Add),
            (2, TransferAction::Keep),
            (3, TransferAction::Add),
        ],
        "conflicts keep the destination, adds are copied"
    );
    assert_eq!(
        preview.custom_lists,
        ["Existing", "Favorites"],
        "both platforms' names are merged"
    );

    let progress = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&progress);
    run_transfer(&preview, &destination, |step| {
        seen.lock().unwrap().push(step);
    })
    .await
    .unwrap();

    let state = destination.snapshot();
    assert_eq!(state.entries.len(), 3, "both new entries were added");
    assert_eq!(
        state.entries[&media(2)].notes,
        "kept",
        "the destination's conflict is untouched"
    );
    assert_eq!(state.entries[&media(1)].notes, "new a");
    assert_eq!(state.custom_lists, ["Existing", "Favorites"]);
    assert_eq!(
        *progress.lock().unwrap(),
        vec![
            TransferProgress { done: 1, total: 2 },
            TransferProgress { done: 2, total: 2 },
        ]
    );

    let overridden = FakePlatform::with(vec![entry(2, "kept", &[])]);
    let mut preview = plan(&source, &overridden);
    if let Some(line) = preview
        .entries
        .iter_mut()
        .find(|line| line.media() == media(2))
    {
        line.action = TransferAction::Replace;
    }
    run_transfer(&preview, &overridden, |_| {}).await.unwrap();
    let state = overridden.snapshot();
    assert_eq!(
        state.entries[&media(2)].notes,
        "new b",
        "an explicit override replaces the destination"
    );
    assert_eq!(state.entries.len(), 3);
}

#[tokio::test]
async fn re_run_resumes_without_repeating_copied_entries() {
    let source = FakePlatform::with(vec![
        entry(1, "a", &[]),
        entry(2, "b", &[]),
        entry(3, "c", &[]),
    ]);
    let destination = FakePlatform::with(Vec::new());
    destination.state.lock().unwrap().fail_on_save = Some(2);

    let preview = plan(&source, &destination);
    assert!(
        run_transfer(&preview, &destination, |_| {}).await.is_err(),
        "the injected interruption stops the run"
    );
    assert_eq!(destination.snapshot().entries.len(), 1, "one entry landed");
    assert_eq!(destination.snapshot().saves, 2, "the second save failed");

    destination.state.lock().unwrap().fail_on_save = None;
    let preview = plan(&source, &destination);
    assert_eq!(
        preview.pending().len(),
        2,
        "re-planning skips the already copied entry"
    );
    run_transfer(&preview, &destination, |_| {}).await.unwrap();

    let state = destination.snapshot();
    assert_eq!(state.entries.len(), 3, "all source entries are present");
    assert_eq!(state.entries[&media(1)].notes, "a");
    assert_eq!(state.entries[&media(2)].notes, "b");
    assert_eq!(state.entries[&media(3)].notes, "c");
    assert_eq!(state.saves, 4);
}

#[tokio::test]
async fn transfer_never_deletes_destination_entries() {
    let source = FakePlatform::with(vec![entry(1, "a", &["New"]), entry(2, "b", &["New"])]);
    let destination = FakePlatform::with(vec![entry(8, "only here", &["Old"])]);

    let preview = plan(&source, &destination);
    run_transfer(&preview, &destination, |_| {}).await.unwrap();

    let state = destination.snapshot();
    assert_eq!(state.deletes, 0, "the runner never deletes anything");
    assert_eq!(
        state.entries.len(),
        3,
        "the destination-only entry survives"
    );
    assert_eq!(state.entries[&media(8)].notes, "only here");
    assert_eq!(state.entries[&media(8)].custom_lists, ["Old"]);
    assert_eq!(state.custom_lists, ["New", "Old"]);
}
