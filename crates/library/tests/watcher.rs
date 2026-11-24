//! Shelving, unshelving and folder-scoped deletion tracer bullets.

use std::{fs, slice};

use domain::{ExternalId, MediaKind};
use library::Library;
use support::{candidate, database, release};

#[cfg(test)]
mod support {
    use std::{fs, path::Path};

    use db::Db;
    use domain::ExternalId;
    use library::Candidate;
    use tempfile::tempdir;

    pub(crate) fn candidate(title: &str, anilist_id: u32) -> Candidate {
        Candidate::new(title, vec![ExternalId::AniList(anilist_id)])
    }

    pub(crate) fn release(folder: &Path, title: &str, episode: u32) -> std::path::PathBuf {
        let path = folder.join(format!("[Group] {title} - {episode:02} [1080p].mkv"));
        fs::write(&path, b"").unwrap();
        path
    }

    pub(crate) async fn database() -> (tempfile::TempDir, Db) {
        let dir = tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        (dir, db)
    }
}

#[tokio::test]
async fn a_missing_drive_shelves_files_and_keeps_their_matches() {
    let (dir, db) = database().await;
    let folder = dir.path().join("drive");
    fs::create_dir_all(&folder).unwrap();
    let path = release(&folder, "Series Alpha", 1);
    let library = Library::new(db.clone());
    library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Alpha", 1001)],
            |_| {},
        )
        .await
        .unwrap();
    library.lock_file(&path).await.unwrap();
    let media = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(1001)])
        .await
        .unwrap()
        .unwrap();

    fs::remove_dir_all(&folder).unwrap();
    let summary = library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Bravo", 2002).with_alias("Series Alpha")],
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!(summary.shelved, 1);
    assert_eq!(summary.files_walked, 0, "a missing drive is not walked");
    let rows = db.local_file().all().await.unwrap();
    let row = rows.iter().find(|row| row.path == path).unwrap();
    assert!(row.shelved);
    assert_eq!(row.media_id, Some(media), "the match is kept");
    assert!(row.locked, "the lock is kept");
}

#[tokio::test]
async fn a_returning_drive_unshelves_without_rematching() {
    let (dir, db) = database().await;
    let folder = dir.path().join("drive");
    fs::create_dir_all(&folder).unwrap();
    let path = release(&folder, "Series Alpha", 1);
    let library = Library::new(db.clone());
    library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Alpha", 1001)],
            |_| {},
        )
        .await
        .unwrap();
    let media = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(1001)])
        .await
        .unwrap()
        .unwrap();

    fs::remove_file(&path).unwrap();
    fs::remove_dir(&folder).unwrap();
    library
        .scan(slice::from_ref(&folder), &[], |_| {})
        .await
        .unwrap();
    fs::create_dir_all(&folder).unwrap();
    fs::write(&path, b"").unwrap();
    let summary = library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Bravo", 2002).with_alias("Series Alpha")],
            |_| {},
        )
        .await
        .unwrap();

    assert_eq!(summary.shelved, 0);
    let rows = db.local_file().all().await.unwrap();
    let row = rows.iter().find(|row| row.path == path).unwrap();
    assert!(!row.shelved);
    assert_eq!(
        row.media_id,
        Some(media),
        "the old match survived the return"
    );
}

#[tokio::test]
async fn deleting_a_file_from_a_present_folder_deletes_its_row() {
    let (dir, db) = database().await;
    let folder = dir.path().join("drive");
    fs::create_dir_all(&folder).unwrap();
    let gone = release(&folder, "Series Alpha", 1);
    let kept = release(&folder, "Series Bravo", 1);
    let library = Library::new(db.clone());
    library
        .scan(
            slice::from_ref(&folder),
            &[
                candidate("Series Alpha", 1001),
                candidate("Series Bravo", 1002),
            ],
            |_| {},
        )
        .await
        .unwrap();

    fs::remove_file(&gone).unwrap();
    library
        .scan(
            slice::from_ref(&folder),
            &[
                candidate("Series Alpha", 1001),
                candidate("Series Bravo", 1002),
            ],
            |_| {},
        )
        .await
        .unwrap();

    let rows = db.local_file().all().await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows.first().unwrap().path, kept);
}
