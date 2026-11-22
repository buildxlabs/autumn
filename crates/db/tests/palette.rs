//! The palette's local data on a temp database (#63).

use std::time::SystemTime;

use db::{CatalogTitle, Db};
use domain::{AdvancedScores, ExternalId, FuzzyDate, ListEntry, ListStatus, MediaKind, Score};
use tempfile::tempdir;

fn title(id: u32, kind: MediaKind, name: &str) -> CatalogTitle {
    CatalogTitle {
        anilist_id: id,
        kind,
        title: name.to_owned(),
        aliases: vec![format!("{name} alias")],
    }
}

fn entry(media: domain::MediaId) -> ListEntry {
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

#[expect(
    clippy::unwrap_used,
    reason = "test helper: a failed mint is a failed test"
)]
async fn mint(db: &Db, kind: MediaKind, id: u32) -> domain::MediaId {
    db.media()
        .find_or_mint(kind, &[ExternalId::AniList(id)])
        .await
        .unwrap()
        .media
}

#[tokio::test]
async fn cached_titles_only_match_listed_media_of_the_same_kind() {
    let dir = tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    let listed = mint(&db, MediaKind::Anime, 154_587).await;
    mint(&db, MediaKind::Anime, 1).await;
    let manga = mint(&db, MediaKind::Manga, 30_013).await;
    db.local_list().save(&entry(listed)).await.unwrap();
    db.local_list().save(&entry(manga)).await.unwrap();

    db.palette()
        .remember(&[
            title(154_587, MediaKind::Anime, "Frieren"),
            title(1, MediaKind::Anime, "Cowboy Bebop"),
            title(30_013, MediaKind::Manga, "Frieren manga"),
        ])
        .await
        .unwrap();

    let anime = db.palette().local(MediaKind::Anime).await.unwrap();
    assert_eq!(anime, [title(154_587, MediaKind::Anime, "Frieren")]);
    assert_eq!(
        db.palette().local(MediaKind::Manga).await.unwrap(),
        [title(30_013, MediaKind::Manga, "Frieren manga")]
    );
}

#[tokio::test]
async fn recents_are_deduplicated_pruned_and_survive_a_reopen() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("autumn.db");
    let db = Db::open(&path).await.unwrap();
    db.palette()
        .remember(&[title(154_587, MediaKind::Anime, "Frieren")])
        .await
        .unwrap();

    for query in ["frieren", "naruto", "frieren", "one piece"] {
        db.palette().record_search(query).await.unwrap();
        // Give each row a distinct second so the order is deterministic.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    db.palette()
        .record_view(154_587, MediaKind::Anime)
        .await
        .unwrap();
    db.palette()
        .record_view(154_587, MediaKind::Anime)
        .await
        .unwrap();
    db.close().await;

    let reopened = Db::open(&path).await.unwrap();
    assert_eq!(
        reopened.palette().recent_searches().await.unwrap(),
        ["one piece", "frieren", "naruto"],
        "repeated queries move to the top"
    );
    assert_eq!(
        reopened
            .palette()
            .recent_views(MediaKind::Anime)
            .await
            .unwrap(),
        [title(154_587, MediaKind::Anime, "Frieren")]
    );
    assert_eq!(
        reopened
            .palette()
            .recent_views(MediaKind::Manga)
            .await
            .unwrap(),
        Vec::<CatalogTitle>::new(),
        "kinds do not share recents"
    );
}

#[tokio::test]
async fn a_view_without_a_cached_title_is_not_shown() {
    let dir = tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();

    db.palette()
        .record_view(999, MediaKind::Anime)
        .await
        .unwrap();

    assert_eq!(
        db.palette().recent_views(MediaKind::Anime).await.unwrap(),
        Vec::<CatalogTitle>::new(),
        "a view cannot show a title it never cached"
    );
}

#[tokio::test]
async fn whitespace_only_searches_are_ignored() {
    let dir = tempdir().unwrap();
    let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();

    db.palette().record_search("   ").await.unwrap();

    assert_eq!(
        db.palette().recent_searches().await.unwrap(),
        Vec::<String>::new()
    );
}
