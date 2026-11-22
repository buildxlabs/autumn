//! The account-free Local platform, on a temp database.

use db::Db;
use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaKind, Score};
use support::{entry, fresh, mint, names};
use tracker::{List, ListName, Local, Platform, TrackerError};

#[tokio::test]
async fn an_added_entry_is_fetched_back_with_every_field() {
    let (_dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Manga, 30_013).await;
    let added = ListEntry {
        status: ListStatus::Paused,
        score: Score::new(85).unwrap(),
        progress: 120,
        progress_volumes: 11,
        repeat: 2,
        started_at: FuzzyDate::new(Some(2019), Some(4), None).unwrap(),
        completed_at: FuzzyDate::new(Some(2021), Some(2), Some(28)).unwrap(),
        notes: "Reread the Water 7 arc".to_owned(),
        private: true,
        priority: 3,
        advanced_scores: AdvancedScores {
            story: Score::new(90).unwrap(),
            characters: Score::new(80).unwrap(),
            visuals: Score::new(70).unwrap(),
            audio: Score::UNSCORED,
            enjoyment: Score::new(100).unwrap(),
        },
        ..entry(media)
    };

    let saved = local.save_entry(&added).await.unwrap();
    let collection = local.collection(MediaKind::Manga).await.unwrap();

    assert_eq!(
        ListEntry {
            created_at: added.created_at,
            updated_at: added.updated_at,
            ..saved.clone()
        },
        added,
        "every field but the timestamps is stored as given"
    );
    assert_eq!(
        collection.lists,
        [List {
            name: ListName::Status(ListStatus::Paused),
            entries: vec![saved],
        }]
    );
}

#[tokio::test]
async fn updating_progress_keeps_the_other_fields() {
    let (_dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Anime, 21).await;
    let first = local
        .save_entry(&ListEntry {
            score: Score::new(90).unwrap(),
            notes: "Weekly".to_owned(),
            progress: 3,
            ..entry(media)
        })
        .await
        .unwrap();

    let updated = local.update_progress(media, 4).await.unwrap();

    assert_eq!(updated.progress, 4);
    assert_eq!(
        ListEntry {
            progress: 3,
            updated_at: first.updated_at,
            ..updated
        },
        first,
        "only progress (and updated_at) changes"
    );
}

#[tokio::test]
async fn progress_on_a_media_not_in_the_list_adds_it_as_current() {
    let (_dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Anime, 21).await;

    let added = local.update_progress(media, 1).await.unwrap();

    assert_eq!((added.status, added.progress), (ListStatus::Current, 1));
    assert_eq!(
        names(&local.collection(MediaKind::Anime).await.unwrap()),
        [ListName::Status(ListStatus::Current)]
    );
}

#[tokio::test]
async fn changing_status_moves_the_entry_and_keeps_created_at() {
    let (_dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Anime, 21).await;
    let watching = local.save_entry(&entry(media)).await.unwrap();

    let completed = local
        .save_entry(&ListEntry {
            status: ListStatus::Completed,
            ..watching.clone()
        })
        .await
        .unwrap();

    assert_eq!(completed.created_at, watching.created_at);
    assert_eq!(
        names(&local.collection(MediaKind::Anime).await.unwrap()),
        [ListName::Status(ListStatus::Completed)]
    );
}

#[tokio::test]
async fn the_collection_groups_by_status_then_custom_list() {
    let (_dir, db, local) = fresh().await;
    let a = mint(&db, MediaKind::Anime, 1).await;
    let b = mint(&db, MediaKind::Anime, 2).await;
    let c = mint(&db, MediaKind::Anime, 3).await;
    let d = mint(&db, MediaKind::Anime, 4).await;
    let saved = |entry| {
        let local = local.clone();
        async move { local.save_entry(&entry).await.unwrap() }
    };
    let a = saved(ListEntry {
        status: ListStatus::Planning,
        custom_lists: vec!["Rewatch".to_owned(), "Favourites".to_owned()],
        ..entry(a)
    })
    .await;
    let b = saved(ListEntry {
        status: ListStatus::Dropped,
        custom_lists: vec!["Favourites".to_owned()],
        hidden_from_status_lists: true,
        ..entry(b)
    })
    .await;
    let c = saved(ListEntry {
        status: ListStatus::Repeating,
        ..entry(c)
    })
    .await;
    let d = saved(entry(d)).await;

    let collection = local.collection(MediaKind::Anime).await.unwrap();

    // Order within a list follows updated_at, which has one-second
    // precision, so compare membership only.
    let media = |list: &List| {
        let mut media: Vec<_> = list.entries.iter().map(|e| e.media).collect();
        media.sort();
        media
    };
    let lists: Vec<_> = collection
        .lists
        .iter()
        .map(|list| (list.name.clone(), media(list)))
        .collect();
    assert_eq!(
        lists,
        [
            (ListName::Status(ListStatus::Current), vec![d.media]),
            (ListName::Status(ListStatus::Repeating), vec![c.media]),
            (ListName::Status(ListStatus::Planning), vec![a.media]),
            (
                ListName::Custom("Favourites".to_owned()),
                vec![a.media, b.media]
            ),
            (ListName::Custom("Rewatch".to_owned()), vec![a.media]),
        ],
        "status lists in fixed order, empty ones left out, hidden entry only in its custom list"
    );
    assert_eq!(
        a.custom_lists,
        ["Favourites", "Rewatch"],
        "custom lists read back sorted"
    );
}

#[tokio::test]
async fn setting_custom_lists_replaces_the_old_ones() {
    let (_dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Anime, 21).await;
    let first = local
        .save_entry(&ListEntry {
            custom_lists: vec!["Old".to_owned(), "Kept".to_owned()],
            ..entry(media)
        })
        .await
        .unwrap();

    let second = local
        .save_entry(&ListEntry {
            custom_lists: vec!["Kept".to_owned(), "New".to_owned(), "New".to_owned()],
            ..first
        })
        .await
        .unwrap();

    assert_eq!(second.custom_lists, ["Kept", "New"]);
    assert_eq!(
        names(&local.collection(MediaKind::Anime).await.unwrap()),
        [
            ListName::Status(ListStatus::Current),
            ListName::Custom("Kept".to_owned()),
            ListName::Custom("New".to_owned()),
        ]
    );
}

#[tokio::test]
async fn a_deleted_entry_leaves_every_list() {
    let (_dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Anime, 21).await;
    local
        .save_entry(&ListEntry {
            custom_lists: vec!["Favourites".to_owned()],
            ..entry(media)
        })
        .await
        .unwrap();

    local.delete_entry(media).await.unwrap();
    local.delete_entry(media).await.unwrap();

    assert_eq!(local.collection(MediaKind::Anime).await.unwrap().lists, []);
    let re_added = local.save_entry(&entry(media)).await.unwrap();
    assert_eq!(
        re_added.custom_lists,
        Vec::<String>::new(),
        "membership went too"
    );
}

#[tokio::test]
async fn each_kind_has_its_own_collection() {
    let (_dir, db, local) = fresh().await;
    let anime = mint(&db, MediaKind::Anime, 21).await;
    let manga = mint(&db, MediaKind::Manga, 30_013).await;
    local
        .save_entry(&ListEntry {
            custom_lists: vec!["Favourites".to_owned()],
            ..entry(anime)
        })
        .await
        .unwrap();
    let manga_entry = local.save_entry(&entry(manga)).await.unwrap();

    let manga_lists = local.collection(MediaKind::Manga).await.unwrap();

    assert_eq!(
        manga_lists.lists,
        [List {
            name: ListName::Status(ListStatus::Current),
            entries: vec![manga_entry],
        }]
    );
}

#[tokio::test]
async fn saving_an_unknown_media_fails() {
    let (_dir, _db, local) = fresh().await;
    let unknown = domain::MediaId::new(999);

    let error = local.save_entry(&entry(unknown)).await.unwrap_err();

    assert!(
        matches!(error, TrackerError::UnknownMedia(media) if media == unknown),
        "{error:?}"
    );
}

#[tokio::test]
async fn entries_survive_closing_and_reopening_the_database() {
    let (dir, db, local) = fresh().await;
    let media = mint(&db, MediaKind::Anime, 21).await;
    let saved = local
        .save_entry(&ListEntry {
            progress: 7,
            custom_lists: vec!["Favourites".to_owned()],
            ..entry(media)
        })
        .await
        .unwrap();
    drop(local);
    db.close().await;

    let reopened = Local::new(Db::open(&dir.path().join("autumn.db")).await.unwrap());
    let collection = reopened.collection(MediaKind::Anime).await.unwrap();

    assert_eq!(
        collection.lists,
        [
            List {
                name: ListName::Status(ListStatus::Current),
                entries: vec![saved.clone()],
            },
            List {
                name: ListName::Custom("Favourites".to_owned()),
                entries: vec![saved],
            },
        ]
    );
}

/// Test helpers; inside `cfg(test)` so the workspace's test-only lint
/// allowances apply.
#[cfg(test)]
mod support {
    use std::time::SystemTime;

    use db::Db;
    use domain::{
        AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
    };
    use tempfile::TempDir;
    use tracker::{Collection, ListName, Local};

    pub(crate) async fn fresh() -> (TempDir, Db, Local) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        let local = Local::new(db.clone());
        (dir, db, local)
    }

    pub(crate) async fn mint(db: &Db, kind: MediaKind, anilist_id: u32) -> MediaId {
        db.media()
            .find_or_mint(kind, &[ExternalId::AniList(anilist_id)])
            .await
            .unwrap()
            .media
    }

    /// A Current entry with nothing else set.
    pub(crate) fn entry(media: MediaId) -> ListEntry {
        ListEntry {
            media,
            status: ListStatus::Current,
            score: Score::UNSCORED,
            progress: 0,
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
            created_at: SystemTime::UNIX_EPOCH,
            updated_at: SystemTime::UNIX_EPOCH,
        }
    }

    /// The names of a collection's lists, in order.
    pub(crate) fn names(collection: &Collection) -> Vec<ListName> {
        collection
            .lists
            .iter()
            .map(|list| list.name.clone())
            .collect()
    }
}
