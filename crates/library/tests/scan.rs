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
