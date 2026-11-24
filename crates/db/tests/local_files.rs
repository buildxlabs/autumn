//! Local-file persistence and the Phase 2 cross-reference providers.

use db::{Db, LocalFile};
use domain::{ExternalId, MediaId, MediaKind};

#[tokio::test]
async fn a_rescan_updates_rows_in_place_and_keeps_media_ids() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let media = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
        .await
        .unwrap()
        .media;

    let first = file("show/- 01.mkv", 100, Some(media));
    let gone = file("show/- 02.mkv", 200, Some(media));
    db.local_file().sync(&[first.clone(), gone]).await.unwrap();

    let changed = LocalFile {
        size: 150,
        ..first.clone()
    };
    let added = file("other/- 01.mkv", 300, None);
    db.local_file()
        .sync(&[changed.clone(), added.clone()])
        .await
        .unwrap();

    let rows = db.local_file().all().await.unwrap();
    assert_eq!(rows.len(), 2, "one removed, one added");
    assert!(rows.contains(&changed), "the row was updated in place");
    assert!(rows.contains(&added), "the new file was added");
    assert_eq!(
        rows.iter()
            .find(|row| row.path == first.path)
            .and_then(|row| row.media_id),
        Some(media),
        "the existing match survives"
    );
}

#[tokio::test]
async fn anidb_and_kitsu_ids_are_cross_references() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let ids = [ExternalId::AniDb(7), ExternalId::Kitsu(9)];

    let minted = db
        .media()
        .find_or_mint(MediaKind::Anime, &ids)
        .await
        .unwrap();
    assert_eq!(minted.conflicts.len(), 0);

    let found = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::Kitsu(9)])
        .await
        .unwrap();
    assert_eq!(found, Some(minted.media), "Kitsu reuses the same media");
    assert_eq!(
        db.media().external_ids(minted.media).await.unwrap(),
        ids,
        "every id is stored"
    );
}

fn file(path: &str, size: i64, media: Option<MediaId>) -> LocalFile {
    LocalFile {
        path: std::path::PathBuf::from(path),
        size,
        modified: 1_700_000_000,
        title: Some("Show".to_owned()),
        group: Some("Group".to_owned()),
        season: None,
        episode: Some(1),
        kind: "episode".to_owned(),
        media_id: media,
        confidence: media.map(|_| 0.9),
        locked: false,
        ignored: false,
        filler: false,
        shelved: false,
    }
}
