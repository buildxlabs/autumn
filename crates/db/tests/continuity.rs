//! Resume positions survive a database restart and never leak across episodes.

use db::Db;
use domain::{ExternalId, MediaId, MediaKind};

#[cfg(test)]
async fn media(db: &Db, anilist_id: u32) -> MediaId {
    db.media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(anilist_id)])
        .await
        .unwrap()
        .media
}

#[tokio::test]
async fn a_saved_position_is_keyed_by_media_and_episode() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let show = media(&db, 1).await;
    let other = media(&db, 2).await;

    db.continuity()
        .save(show, 3, 120.5, Some(1440.0))
        .await
        .unwrap();
    db.continuity().save(other, 3, 10.0, None).await.unwrap();
    db.continuity().save(show, 4, 30.0, None).await.unwrap();

    let saved = db.continuity().get(show, 3).await.unwrap().unwrap();
    assert_eq!(saved.position, 120.5);
    assert_eq!(saved.duration, Some(1440.0));
    assert_eq!(db.continuity().get(show, 5).await.unwrap(), None);
}

#[tokio::test]
async fn saving_updates_the_row_and_clearing_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("autumn.db");
    let show = {
        let db = Db::open(&path).await.unwrap();
        let show = media(&db, 1).await;
        db.continuity()
            .save(show, 1, 20.0, Some(100.0))
            .await
            .unwrap();
        db.continuity()
            .save(show, 1, 40.0, Some(100.0))
            .await
            .unwrap();
        show
    };

    let reopened = Db::open(&path).await.unwrap();
    let saved = reopened.continuity().get(show, 1).await.unwrap().unwrap();
    assert_eq!(saved.position, 40.0, "the later position won");

    reopened.continuity().clear(show, 1).await.unwrap();
    assert_eq!(
        reopened.continuity().get(show, 1).await.unwrap(),
        None,
        "a watched episode has no resume position"
    );
}

#[tokio::test]
async fn an_invalid_position_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let show = media(&db, 1).await;

    let result = db.continuity().save(show, 1, f64::NAN, None).await;
    assert!(result.is_err(), "a NaN position cannot be stored");
}

#[tokio::test]
async fn a_media_lists_its_saved_positions_in_episode_order() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let show = media(&db, 1).await;
    let other = media(&db, 2).await;
    db.continuity().save(show, 4, 30.0, None).await.unwrap();
    db.continuity()
        .save(show, 2, 120.0, Some(1440.0))
        .await
        .unwrap();
    db.continuity().save(other, 3, 10.0, None).await.unwrap();

    let saved = db.continuity().for_media(show).await.unwrap();

    let episodes: Vec<u32> = saved.iter().map(|(episode, _)| *episode).collect();
    assert_eq!(episodes, [2, 4], "only this media's rows, by episode");
    assert_eq!(saved.first().unwrap().1.duration, Some(1440.0));
}
