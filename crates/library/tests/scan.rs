//! Scan tracer bullet: folders in, matched local files out and persisted.

use std::{fs, slice};

use domain::{ExternalId, MediaKind};
use library::{Library, ScanProgress};
use support::{candidates, database, release};

#[tokio::test]
async fn scans_thirty_fansub_files_across_two_folders_and_stores_matches() {
    let (dir, db) = database().await;
    let first = dir.path().join("drive-a");
    let second = dir.path().join("drive-b");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    for index in 0..30 {
        let folder = if index < 15 { &first } else { &second };
        let title = support::TITLES.get(index % support::TITLES.len()).unwrap();
        let episode = u32::try_from(index / support::TITLES.len() + 1).unwrap();
        release(folder, title, episode);
    }

    let mut progress = Vec::new();
    let summary = Library::new(db.clone())
        .scan(&[first, second], &candidates(), |update: ScanProgress| {
            progress.push(update);
        })
        .await
        .unwrap();

    assert_eq!(summary.files_walked, 30);
    assert_eq!(summary.files_matched, 30, "every group found its candidate");
    assert_eq!(summary.files_unmatched, 0);
    assert!(progress.iter().any(|update| update.files_matched == 30));

    let rows = db.local_file().all().await.unwrap();
    assert_eq!(rows.len(), 30);
    for row in rows {
        let index = support::TITLES
            .iter()
            .position(|title| row.title.as_deref() == Some(*title))
            .unwrap();
        let expected = db
            .media()
            .find(
                MediaKind::Anime,
                &[ExternalId::AniList(1000 + u32::try_from(index).unwrap())],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.media_id, Some(expected), "file {}", row.path.display());
        assert!(row.confidence.is_some_and(|value| value > 0.5));
        let name = row.path.file_name().unwrap().to_string_lossy();
        let episode = if name.contains("- 03") {
            3
        } else if name.contains("- 02") {
            2
        } else {
            1
        };
        assert_eq!(row.episode, Some(episode), "file {}", row.path.display());
    }
}

#[tokio::test]
async fn a_rescan_adds_and_removes_files_without_changing_media_ids() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    release(&folder, "Series Alpha", 1);
    release(&folder, "Series Bravo", 1);

    let library = Library::new(db.clone());
    let initial = library
        .scan(slice::from_ref(&folder), &candidates(), |_| {})
        .await
        .unwrap();
    assert_eq!(initial.files_matched, 2);
    let before = db.local_file().all().await.unwrap();
    let alpha = before
        .iter()
        .find(|row| row.title.as_deref() == Some("Series Alpha"))
        .unwrap()
        .media_id;

    fs::remove_file(folder.join("[Group] Series Bravo - 01 [1080p].mkv")).unwrap();
    release(&folder, "Series Alpha", 2);
    release(&folder, "Series Charlie", 1);

    let after = library
        .scan(slice::from_ref(&folder), &candidates(), |_| {})
        .await
        .unwrap();
    assert_eq!(after.files_walked, 3);
    assert_eq!(after.files_matched, 3);

    let rows = db.local_file().all().await.unwrap();
    assert_eq!(rows.len(), 3, "the removed file's row is gone");
    let alpha_after = rows
        .iter()
        .find(|row| row.title.as_deref() == Some("Series Alpha"))
        .unwrap()
        .media_id;
    assert_eq!(alpha_after, alpha, "the existing media id is kept");
    assert!(
        rows.iter()
            .any(|row| row.title.as_deref() == Some("Series Charlie"))
    );
}

#[tokio::test]
async fn an_emptied_folder_leaves_other_folders_and_partial_deletes_alone() {
    let (dir, db) = database().await;
    let internal = dir.path().join("internal");
    let mount = dir.path().join("mnt-anime");
    fs::create_dir_all(&internal).unwrap();
    fs::create_dir_all(&mount).unwrap();
    let gone = internal.join("[Group] Series Alpha - 01 [1080p].mkv");
    let kept = internal.join("[Group] Series Bravo - 01 [1080p].mkv");
    let shelved = mount.join("[Group] Series Charlie - 01 [1080p].mkv");
    release(&internal, "Series Alpha", 1);
    release(&internal, "Series Bravo", 1);
    release(&mount, "Series Charlie", 1);
    let library = Library::new(db.clone());
    let candidates = candidates();
    library
        .scan(&[internal.clone(), mount.clone()], &candidates, |_| {})
        .await
        .unwrap();
    library.lock_file(&kept).await.unwrap();
    library.lock_file(&shelved).await.unwrap();
    let media = |id: u32| {
        let db = db.clone();
        async move {
            db.media()
                .find(MediaKind::Anime, &[ExternalId::AniList(1000 + id)])
                .await
                .unwrap()
                .unwrap()
        }
    };
    let (bravo, charlie) = (media(1).await, media(2).await);

    fs::remove_file(&gone).unwrap();
    fs::remove_file(&shelved).unwrap();
    let summary = library
        .scan(&[internal, mount], &candidates, |_| {})
        .await
        .unwrap();

    assert_eq!(summary.shelved, 1, "only the emptied folder is shelved");
    let rows = db.local_file().all().await.unwrap();
    assert_eq!(rows.len(), 2, "only the partially deleted file is dropped");
    assert!(!rows.iter().any(|row| row.path == gone));
    let row = rows.iter().find(|row| row.path == kept).unwrap();
    assert!(!row.shelved);
    assert!(row.locked);
    assert_eq!(row.media_id, Some(bravo));
    let row = rows.iter().find(|row| row.path == shelved).unwrap();
    assert!(row.shelved);
    assert!(row.locked);
    assert_eq!(row.media_id, Some(charlie));
}

#[tokio::test]
async fn a_file_with_no_title_anywhere_stays_unmatched() {
    let (dir, db) = database().await;
    let folder = dir.path().join("Season 01");
    fs::create_dir_all(&folder).unwrap();
    fs::write(folder.join("- 01.mkv"), b"").unwrap();

    let summary = Library::new(db.clone())
        .scan(slice::from_ref(&folder), &candidates(), |_| {})
        .await
        .unwrap();
    assert_eq!(summary.files_walked, 1);
    assert_eq!(
        summary.files_matched, 0,
        "no title cannot match a candidate"
    );

    let rows = db.local_file().all().await.unwrap();
    let row = rows.first().unwrap();
    assert_eq!(row.media_id, None);
    assert_eq!(row.confidence, None);
}

#[tokio::test]
async fn a_season_marker_in_the_name_picks_that_season() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    fs::write(
        folder.join("[SubsPlease] Mushoku Tensei - 05 (1080p) [5D1F0E2A].mkv"),
        b"",
    )
    .unwrap();
    fs::write(
        folder.join("[SubsPlease] Mushoku Tensei S2 - 01 (1080p) [EC64C8B1].mkv"),
        b"",
    )
    .unwrap();
    let candidates = [
        library::Candidate::new(
            "Mushoku Tensei: Isekai Ittara Honki Dasu",
            vec![ExternalId::AniList(108_465)],
        )
        .with_alias("Mushoku Tensei: Jobless Reincarnation"),
        library::Candidate::new(
            "Mushoku Tensei II: Isekai Ittara Honki Dasu",
            vec![ExternalId::AniList(146_065)],
        )
        .with_alias("Mushoku Tensei: Jobless Reincarnation Season 2"),
    ];

    Library::new(db.clone())
        .scan(slice::from_ref(&folder), &candidates, |_| {})
        .await
        .unwrap();

    let media = |id| {
        let db = db.clone();
        async move {
            db.media()
                .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(id)])
                .await
                .unwrap()
                .media
        }
    };
    let (first, second) = (media(108_465).await, media(146_065).await);
    let rows = db.local_file().all().await.unwrap();
    let season = |marker: &str| {
        rows.iter()
            .find(|row| row.path.to_string_lossy().contains(marker))
            .and_then(|row| row.media_id)
    };
    assert_eq!(
        season("Tensei - 05"),
        Some(first),
        "no marker: the first season"
    );
    assert_eq!(
        season("Tensei S2 - 01"),
        Some(second),
        "S2: the second season"
    );
}

/// Test helpers, in `cfg(test)` so the workspace's test-only lint
/// allowances apply.
#[cfg(test)]
mod support {
    use std::{fs, path::Path};

    use db::Db;
    use domain::ExternalId;
    use library::Candidate;
    use tempfile::tempdir;

    pub(crate) const TITLES: [&str; 10] = [
        "Series Alpha",
        "Series Bravo",
        "Series Charlie",
        "Series Delta",
        "Series Echo",
        "Series Foxtrot",
        "Series Golf",
        "Series Hotel",
        "Series India",
        "Series Juliet",
    ];

    pub(crate) fn candidates() -> Vec<Candidate> {
        TITLES
            .iter()
            .enumerate()
            .map(|(index, title)| {
                Candidate::new(
                    *title,
                    vec![ExternalId::AniList(1000 + u32::try_from(index).unwrap())],
                )
            })
            .collect()
    }

    pub(crate) fn release(folder: &Path, title: &str, episode: u32) {
        fs::write(
            folder.join(format!("[Group] {title} - {episode:02} [1080p].mkv")),
            b"",
        )
        .unwrap();
    }

    pub(crate) async fn database() -> (tempfile::TempDir, Db) {
        let dir = tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        (dir, db)
    }
}
