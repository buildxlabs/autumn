//! Media identity and cross-references (ADR 0001), on a temp database.

use db::{Db, DbError, LinkConflict};
use domain::{ExternalId, MediaKind};
use support::{capture_logs, fresh_db};

#[tokio::test]
async fn migrations_apply_on_a_fresh_file_and_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("autumn.db");

    let db = Db::open(&path).await.unwrap();
    let media = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
        .await
        .unwrap()
        .media;
    db.close().await;
    let reopened = Db::open(&path).await.unwrap();

    let found = reopened
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(1)])
        .await
        .unwrap();
    assert_eq!(found, Some(media), "data survives a reopen");
}

#[tokio::test]
async fn the_database_is_in_wal_mode_after_open() {
    let (dir, db) = fresh_db().await;
    db.close().await;

    let wal = dir.path().join("autumn.db-wal");
    let reopened = Db::open(&dir.path().join("autumn.db")).await.unwrap();
    reopened
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
        .await
        .unwrap();

    assert!(wal.exists(), "a write goes through the -wal sidecar");
    assert_eq!(reopened.journal_mode().await.unwrap(), "wal");
}

#[tokio::test]
async fn lookup_by_unknown_id_finds_nothing_and_mints_nothing() {
    let (_dir, db) = fresh_db().await;

    let found = db
        .media()
        .find(MediaKind::Anime, &[ExternalId::AniList(21)])
        .await
        .unwrap();

    assert_eq!(found, None, "browsing never mints");
}

#[tokio::test]
async fn minting_by_anilist_id_is_idempotent() {
    let (_dir, db) = fresh_db().await;
    let ids = [ExternalId::AniList(21), ExternalId::Mal(21)];

    let first = db
        .media()
        .find_or_mint(MediaKind::Anime, &ids)
        .await
        .unwrap();
    let second = db
        .media()
        .find_or_mint(MediaKind::Anime, &ids)
        .await
        .unwrap();

    assert_eq!(first.media, second.media, "same media both times");
    assert!(first.conflicts.is_empty() && second.conflicts.is_empty());
    assert_eq!(
        db.media()
            .find(MediaKind::Anime, &[ExternalId::Mal(21)])
            .await
            .unwrap(),
        Some(first.media),
        "idMal is linked too"
    );
}

#[tokio::test]
async fn a_known_mal_id_is_reused_when_minting_by_anilist_id() {
    let (_dir, db) = fresh_db().await;
    let by_mal = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::Mal(5114)])
        .await
        .unwrap();

    let both = db
        .media()
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(5114), ExternalId::Mal(5114)],
        )
        .await
        .unwrap();

    assert_eq!(
        both.media, by_mal.media,
        "every known id is looked up first"
    );
}

#[tokio::test]
async fn a_conflicting_mal_link_keeps_the_first_and_logs() {
    let (_dir, db) = fresh_db().await;
    let (logs, _logging) = capture_logs();
    let repo = db.media();
    let first = repo
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(1), ExternalId::Mal(1)],
        )
        .await
        .unwrap();
    let second = repo
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(2)])
        .await
        .unwrap();

    // AniList 2's record later claims idMal 1, which already names media 1.
    let relinked = repo
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(2), ExternalId::Mal(1)],
        )
        .await
        .unwrap();

    assert_eq!(relinked.media, second.media, "found by its AniList id");
    assert_eq!(
        relinked.conflicts,
        [LinkConflict::NamesOtherMedia {
            id: ExternalId::Mal(1),
            other: first.media,
        }],
    );
    assert_eq!(
        repo.find(MediaKind::Anime, &[ExternalId::Mal(1)])
            .await
            .unwrap(),
        Some(first.media),
        "the first link wins"
    );
    assert!(
        logs.text().contains("cross-reference conflict"),
        "logged: {}",
        logs.text()
    );
}

#[tokio::test]
async fn a_second_id_from_the_same_provider_is_a_conflict() {
    let (_dir, db) = fresh_db().await;
    let first = db
        .media()
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(1), ExternalId::Mal(1)],
        )
        .await
        .unwrap();

    let again = db
        .media()
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(1), ExternalId::Mal(99)],
        )
        .await
        .unwrap();

    assert_eq!(again.media, first.media);
    assert_eq!(
        again.conflicts,
        [LinkConflict::ProviderAlreadyLinked {
            id: ExternalId::Mal(99),
        }],
    );
    assert_eq!(
        db.media()
            .find(MediaKind::Anime, &[ExternalId::Mal(99)])
            .await
            .unwrap(),
        None
    );
}

#[tokio::test]
async fn mal_anime_and_manga_ids_do_not_collide() {
    let (_dir, db) = fresh_db().await;
    let repo = db.media();
    // MAL anime 1 is Cowboy Bebop, MAL manga 1 is Monster.
    let bebop = repo
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(1), ExternalId::Mal(1)],
        )
        .await
        .unwrap();

    let monster = repo
        .find_or_mint(
            MediaKind::Manga,
            &[ExternalId::AniList(30001), ExternalId::Mal(1)],
        )
        .await
        .unwrap();

    assert_ne!(monster.media, bebop.media, "two different works");
    assert!(
        monster.conflicts.is_empty(),
        "no false conflict: {:?}",
        monster.conflicts
    );
    let manga = repo
        .find(MediaKind::Manga, &[ExternalId::Mal(1)])
        .await
        .unwrap();
    assert_eq!(manga, Some(monster.media), "MAL manga 1 is Monster");
}

#[tokio::test]
async fn an_id_naming_a_media_of_the_other_kind_is_not_reused() {
    let (_dir, db) = fresh_db().await;
    let (logs, _logging) = capture_logs();
    let repo = db.media();
    let anime = repo
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
        .await
        .unwrap()
        .media;

    // A manga record wrongly claims AniList 1, with its own MAL manga id.
    let manga = repo
        .find_or_mint(
            MediaKind::Manga,
            &[ExternalId::AniList(1), ExternalId::Mal(5)],
        )
        .await
        .unwrap();

    assert_ne!(
        manga.media, anime,
        "a manga is never linked to an anime row"
    );
    assert_eq!(
        manga.conflicts,
        [LinkConflict::NamesOtherMedia {
            id: ExternalId::AniList(1),
            other: anime,
        }],
    );
    assert_eq!(
        repo.find(MediaKind::Manga, &[ExternalId::Mal(5)])
            .await
            .unwrap(),
        Some(manga.media),
        "the MAL manga id sits on the manga row"
    );
    assert_eq!(
        repo.find(MediaKind::Manga, &[ExternalId::AniList(1)])
            .await
            .unwrap(),
        None,
        "find never returns a media of the other kind"
    );
    assert!(
        logs.text().contains("names a media of the other kind"),
        "{}",
        logs.text()
    );
}

#[tokio::test]
async fn ids_beyond_phase_1_are_refused() {
    let (_dir, db) = fresh_db().await;

    let err = db
        .media()
        .find_or_mint(MediaKind::Anime, &[ExternalId::Kitsu(1)])
        .await
        .unwrap_err();

    assert!(
        matches!(err, DbError::UnsupportedExternalId(_)),
        "got {err:?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_first_records_mint_one_media() {
    let (_dir, db) = fresh_db().await;
    let ids = [ExternalId::AniList(21), ExternalId::Mal(21)];

    let mints = (0..8).map(|_| {
        let db = db.clone();
        let ids = ids.clone();
        tokio::spawn(async move { db.media().find_or_mint(MediaKind::Anime, &ids).await })
    });
    let mut minted = Vec::new();
    for mint in mints.collect::<Vec<_>>() {
        minted.push(mint.await.unwrap().unwrap().media);
    }

    minted.dedup();
    assert_eq!(
        minted.len(),
        1,
        "every caller got the same media: {minted:?}"
    );
}

#[tokio::test]
async fn minting_needs_at_least_one_external_id() {
    let (_dir, db) = fresh_db().await;

    let err = db
        .media()
        .find_or_mint(MediaKind::Anime, &[])
        .await
        .unwrap_err();

    assert!(matches!(err, DbError::NoExternalIds), "got {err:?}");
}

#[tokio::test]
async fn flag_then_relink_round_trips() {
    let (_dir, db) = fresh_db().await;
    let media = db
        .media()
        .find_or_mint(
            MediaKind::Anime,
            &[ExternalId::AniList(100), ExternalId::Mal(7)],
        )
        .await
        .unwrap()
        .media;

    db.media().flag_missing_from_anilist(media).await.unwrap();
    assert!(
        db.media().is_missing_from_anilist(media).await.unwrap(),
        "flagged"
    );

    db.media().relink_anilist(media, 200).await.unwrap();

    assert!(
        !db.media().is_missing_from_anilist(media).await.unwrap(),
        "flag cleared"
    );
    let repo = db.media();
    assert_eq!(
        repo.find(MediaKind::Anime, &[ExternalId::AniList(200)])
            .await
            .unwrap(),
        Some(media)
    );
    assert_eq!(
        repo.find(MediaKind::Anime, &[ExternalId::AniList(100)])
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        repo.find(MediaKind::Anime, &[ExternalId::Mal(7)])
            .await
            .unwrap(),
        Some(media),
        "kept"
    );
}

#[tokio::test]
async fn relinking_to_an_anilist_id_another_media_owns_is_refused() {
    let (_dir, db) = fresh_db().await;
    let repo = db.media();
    let a = repo
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(1)])
        .await
        .unwrap()
        .media;
    let b = repo
        .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(2)])
        .await
        .unwrap()
        .media;

    let err = repo.relink_anilist(a, 2).await.unwrap_err();

    assert!(
        matches!(err, DbError::ExternalIdTaken { other, .. } if other == b),
        "got {err:?}"
    );
    assert_eq!(
        repo.find(MediaKind::Anime, &[ExternalId::AniList(1)])
            .await
            .unwrap(),
        Some(a),
        "unchanged"
    );
}

/// Test helpers; inside `cfg(test)` so the workspace's test-only lint
/// allowances apply.
#[cfg(test)]
mod support {
    use std::sync::{Arc, Mutex};

    use db::Db;
    use tempfile::TempDir;

    pub(crate) async fn fresh_db() -> (TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        (dir, db)
    }

    /// Collects `tracing` output on this thread while the guard lives.
    pub(crate) fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    #[derive(Clone, Default)]
    pub(crate) struct Logs(Arc<Mutex<Vec<u8>>>);

    impl Logs {
        pub(crate) fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
}
