//! Library overrides and scan-summary tracer bullets.

use std::{fs, slice};

use domain::{ExternalId, MediaKind};
use library::{Library, UnmatchedReason};
use support::{candidate, database, release};

#[tokio::test]
async fn locking_a_media_locks_all_its_files_and_keeps_their_episodes() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    let first = release(&folder, "Series Alpha", 1);
    let second = release(&folder, "Series Alpha", 2);
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
    assert_eq!(library.lock_media(media).await.unwrap(), 2);

    // The alias would move both files to the other media if they were unlocked.
    library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Bravo", 2002).with_alias("Series Alpha")],
            |_| {},
        )
        .await
        .unwrap();
    let rows = db.local_file().all().await.unwrap();
    for path in [first, second] {
        let row = rows.iter().find(|row| row.path == path).unwrap();
        assert_eq!(row.media_id, Some(media));
        assert!(row.locked);
        let expected = if path.file_name().unwrap().to_string_lossy().contains("- 01") {
            1
        } else {
            2
        };
        assert_eq!(row.episode, Some(expected), "the episode number is kept");
    }
    assert_eq!(library.unlock_media(media).await.unwrap(), 2);
    let rows = db.local_file().all().await.unwrap();
    assert!(rows.iter().all(|row| !row.locked));
}

#[tokio::test]
async fn a_manual_match_unignores_the_file() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    let path = release(&folder, "Series Alpha", 1);
    let library = Library::new(db.clone());
    library
        .scan(slice::from_ref(&folder), &[], |_| {})
        .await
        .unwrap();
    assert_eq!(library.ignore_file(&path).await.unwrap(), 1);

    assert_eq!(
        library
            .manual_match(slice::from_ref(&path), 42, None)
            .await
            .unwrap(),
        1
    );

    let rows = db.local_file().all().await.unwrap();
    let row = rows.iter().find(|row| row.path == path).unwrap();
    assert!(
        row.media_id.is_some(),
        "the assignment survives the next scan"
    );
    assert!(!row.ignored);
    assert!(row.locked);
}

#[tokio::test]
async fn an_override_survives_a_scan_of_a_different_folder() {
    let (dir, db) = database().await;
    let first = dir.path().join("first");
    let second = dir.path().join("second");
    fs::create_dir_all(&first).unwrap();
    fs::create_dir_all(&second).unwrap();
    let locked = release(&first, "Series Alpha", 1);
    let ignored = release(&first, "Series Bravo", 1);
    release(&second, "Series Charlie", 1);
    let candidates = [
        candidate("Series Alpha", 1001),
        candidate("Series Bravo", 1002),
        candidate("Series Charlie", 1003),
    ];
    let library = Library::new(db.clone());
    library
        .scan(slice::from_ref(&first), &candidates, |_| {})
        .await
        .unwrap();
    library.lock_file(&locked).await.unwrap();
    library.ignore_file(&ignored).await.unwrap();

    library.scan(&[second], &candidates, |_| {}).await.unwrap();

    let rows = db.local_file().all().await.unwrap();
    assert!(rows.iter().any(|row| row.path == locked && row.locked));
    assert!(rows.iter().any(|row| row.path == ignored && row.ignored));
}

#[tokio::test]
async fn a_below_threshold_file_keeps_its_best_score_and_candidate() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    release(&folder, "Completely Different Show", 1);
    let library = Library::new(db.clone());
    let live = library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Bravo", 1002)],
            |_| {},
        )
        .await
        .unwrap();
    let reason = live.unmatched.first().map(|file| &file.reason);
    assert!(
        matches!(reason, Some(UnmatchedReason::BelowThreshold { .. })),
        "the file should be below the threshold"
    );
    if let Some(UnmatchedReason::BelowThreshold {
        best_score,
        best_candidate,
    }) = reason
    {
        assert_eq!(best_candidate, "Series Bravo");
        assert!(*best_score > 0.0 && *best_score <= 0.5);
    }

    let stored = library.latest_summary().await.unwrap().unwrap();
    assert_eq!(
        stored.unmatched, live.unmatched,
        "the reason survives storage"
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
async fn a_locked_file_keeps_its_media_when_a_rescan_would_pick_another() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
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
    let alpha = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(1001)])
        .await
        .unwrap()
        .unwrap();
    library.lock_file(&path).await.unwrap();

    // Without the lock, the alias would make this file match the other media.
    library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Bravo", 2002).with_alias("Series Alpha")],
            |_| {},
        )
        .await
        .unwrap();

    let rows = db.local_file().all().await.unwrap();
    let row = rows.iter().find(|row| row.path == path).unwrap();
    assert_eq!(row.media_id, Some(alpha), "the locked match survives");
    assert!(row.locked, "the lock survives a re-scan");
}

#[tokio::test]
async fn an_ignored_file_is_never_matched() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
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
    library.ignore_file(&path).await.unwrap();
    library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Alpha", 1001)],
            |_| {},
        )
        .await
        .unwrap();

    let rows = db.local_file().all().await.unwrap();
    let row = rows.iter().find(|row| row.path == path).unwrap();
    assert_eq!(row.media_id, None, "the ignored file stays unmatched");
    assert!(row.ignored, "the ignored flag survives a re-scan");
}

#[tokio::test]
async fn a_manual_match_mints_the_media_and_locks_the_file() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    let path = release(&folder, "Series Alpha", 1);
    let library = Library::new(db.clone());
    library
        .scan(slice::from_ref(&folder), &[], |_| {})
        .await
        .unwrap();

    let changed = library
        .manual_match(slice::from_ref(&path), 42, Some(7))
        .await
        .unwrap();
    assert_eq!(changed, 1, "one existing file was matched");

    let media = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(42)])
        .await
        .unwrap()
        .unwrap();
    let rows = db.local_file().all().await.unwrap();
    let row = rows.iter().find(|row| row.path == path).unwrap();
    assert_eq!(row.media_id, Some(media), "ADR 0001 minted the media");
    assert_eq!(row.episode, Some(7));
    assert!(row.locked, "a manual match locks the file");

    library
        .scan(
            slice::from_ref(&folder),
            &[candidate("Series Bravo", 2002).with_alias("Series Alpha")],
            |_| {},
        )
        .await
        .unwrap();
    let rows = db.local_file().all().await.unwrap();
    assert_eq!(
        rows.iter().find(|row| row.path == path).unwrap().media_id,
        Some(media),
        "a manual match survives a re-scan"
    );
}

#[tokio::test]
async fn the_latest_summary_reports_matches_reasons_and_counts() {
    let (dir, db) = database().await;
    let folder = dir.path().join("Season 01");
    fs::create_dir_all(&folder).unwrap();
    release(&folder, "Series Alpha", 1);
    let no_title = folder.join("Season 02");
    fs::create_dir_all(&no_title).unwrap();
    fs::write(no_title.join("- 01.mkv"), b"").unwrap();
    let below = release(&folder, "Completely Different Show", 1);
    let ignored = release(&folder, "Series Delta", 1);

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
    library.ignore_file(&ignored).await.unwrap();
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

    let summary = library.latest_summary().await.unwrap().unwrap();
    assert_eq!(summary.folders, vec![folder.clone()]);
    assert_eq!(summary.files_walked, 4);
    assert_eq!(summary.files_matched, 1);
    assert_eq!(summary.ignored, 1);
    assert_eq!(summary.locked, 0);
    assert_eq!(summary.shelved, 0);
    assert_eq!(summary.by_media.len(), 1);
    let entry = summary.by_media.first().unwrap();
    assert_eq!(
        entry.media,
        db.media()
            .find(MediaKind::Anime, &[ExternalId::AniList(1001)])
            .await
            .unwrap()
            .unwrap()
    );
    assert_eq!(entry.files, 1);
    assert!(entry.lowest_confidence > 0.5);

    let reason = |path: &std::path::Path| {
        summary
            .unmatched
            .iter()
            .find(|file| file.path == path)
            .unwrap()
            .reason
            .clone()
    };
    assert!(
        summary
            .unmatched
            .iter()
            .any(|file| file.path == no_title.join("- 01.mkv")),
        "the file without a title in its name is listed as unmatched"
    );
    assert!(matches!(
        reason(&below),
        UnmatchedReason::BelowThreshold { .. }
    ));
    assert_eq!(reason(&ignored), UnmatchedReason::Ignored);
}

#[tokio::test]
async fn a_no_candidate_file_reports_that_reason() {
    let (dir, db) = database().await;
    let folder = dir.path().join("shows");
    fs::create_dir_all(&folder).unwrap();
    release(&folder, "Series Alpha", 1);

    Library::new(db.clone())
        .scan(slice::from_ref(&folder), &[], |_| {})
        .await
        .unwrap();

    let summary = Library::new(db).latest_summary().await.unwrap().unwrap();
    assert!(matches!(
        summary.unmatched.first().map(|file| &file.reason),
        Some(UnmatchedReason::NoCandidate)
    ));
}

#[tokio::test]
async fn the_twenty_first_summary_evicts_the_oldest() {
    let (dir, db) = database().await;
    let library = Library::new(db.clone());
    let candidates = [candidate("Series Alpha", 1001)];
    let mut folders = Vec::new();

    for index in 0..21 {
        let folder = dir.path().join(format!("shows-{index}"));
        fs::create_dir_all(&folder).unwrap();
        release(&folder, "Series Alpha", 1);
        library
            .scan(slice::from_ref(&folder), &candidates, |_| {})
            .await
            .unwrap();
        folders.push(folder);
    }

    assert_eq!(
        db.scan_summary().count().await.unwrap(),
        20,
        "only the last twenty summaries are kept"
    );
    let summaries = library.summaries().await.unwrap();
    assert_eq!(summaries.len(), 20);
    assert_eq!(
        summaries.first().map(|summary| &summary.folders),
        Some(&vec![folders.last().unwrap().clone()])
    );
    assert!(
        summaries
            .iter()
            .all(|summary| summary.folders != vec![folders.first().unwrap().clone()]),
        "the oldest summary was evicted"
    );
}
