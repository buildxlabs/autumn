//! Availability results survive a database restart (#251).

use std::time::{Duration, UNIX_EPOCH};

use db::{AvailabilityIndexer, AvailabilityRecord, Db};
use domain::{ExternalId, MediaKind};

#[cfg(test)]
async fn media(db: &Db, anilist_id: u32) -> i64 {
    db.media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(anilist_id)])
        .await
        .unwrap()
        .media
        .get()
}

#[tokio::test]
async fn saved_checks_are_read_back_after_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("autumn.db");
    let (show, other, checked_at) = {
        let db = Db::open(&path).await.unwrap();
        let show = media(&db, 1).await;
        let other = media(&db, 2).await;
        let checked_at = UNIX_EPOCH + Duration::from_hours(500_000);
        db.availability()
            .save(&[
                AvailabilityRecord {
                    media: show,
                    episode: 12,
                    indexer: AvailabilityIndexer::Nyaa,
                    found: true,
                    checked_at,
                },
                AvailabilityRecord {
                    media: show,
                    episode: 12,
                    indexer: AvailabilityIndexer::AnimeTosho,
                    found: false,
                    checked_at,
                },
                AvailabilityRecord {
                    media: other,
                    episode: 12,
                    indexer: AvailabilityIndexer::Nyaa,
                    found: true,
                    checked_at,
                },
            ])
            .await
            .unwrap();
        (show, other, checked_at)
    };

    let reopened = Db::open(&path).await.unwrap();
    let saved = reopened.availability().for_media(show).await.unwrap();
    assert_eq!(saved.len(), 2, "both indexers were checked");
    assert!(saved.contains(&AvailabilityRecord {
        media: show,
        episode: 12,
        indexer: AvailabilityIndexer::Nyaa,
        found: true,
        checked_at,
    }));
    assert!(saved.contains(&AvailabilityRecord {
        media: show,
        episode: 12,
        indexer: AvailabilityIndexer::AnimeTosho,
        found: false,
        checked_at,
    }));
    assert!(
        reopened
            .availability()
            .for_media(other)
            .await
            .unwrap()
            .iter()
            .all(|row| row.media == other),
        "results do not leak across media"
    );
}

#[tokio::test]
async fn an_older_check_does_not_overwrite_a_newer_one() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let show = media(&db, 1).await;
    let first = UNIX_EPOCH + Duration::from_hours(500_000);
    let later = first + Duration::from_mins(5);
    let row = |found, checked_at| AvailabilityRecord {
        media: show,
        episode: 3,
        indexer: AvailabilityIndexer::Nyaa,
        found,
        checked_at,
    };

    db.availability().save(&[row(false, first)]).await.unwrap();
    db.availability().save(&[row(true, later)]).await.unwrap();
    db.availability().save(&[row(false, first)]).await.unwrap();

    assert_eq!(
        db.availability().for_media(show).await.unwrap(),
        vec![row(true, later)],
        "the newest answer wins"
    );
}
