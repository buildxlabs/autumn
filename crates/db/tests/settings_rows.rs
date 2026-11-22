//! Settings rows: one JSON value per settings tab.

use support::{fresh_db, row};

#[tokio::test]
async fn a_fresh_database_has_no_rows() {
    let (_dir, db) = fresh_db().await;

    assert_eq!(db.settings().rows().await.unwrap(), []);
}

#[tokio::test]
async fn saved_rows_read_back_sorted_by_tab() {
    let (_dir, db) = fresh_db().await;

    db.settings()
        .save(&[row("player", "{}"), row("appearance", r#"{"a":1}"#)])
        .await
        .unwrap();

    assert_eq!(
        db.settings().rows().await.unwrap(),
        [row("appearance", r#"{"a":1}"#), row("player", "{}")]
    );
}

#[tokio::test]
async fn saving_a_tab_again_replaces_it_and_keeps_others() {
    let (_dir, db) = fresh_db().await;
    let settings = db.settings();
    settings
        .save(&[row("appearance", "1"), row("player", "2")])
        .await
        .unwrap();

    settings.save(&[row("appearance", "3")]).await.unwrap();

    assert_eq!(
        settings.rows().await.unwrap(),
        [row("appearance", "3"), row("player", "2")]
    );
}

#[tokio::test]
async fn invalid_json_is_refused_and_nothing_is_saved() {
    let (_dir, db) = fresh_db().await;

    let result = db
        .settings()
        .save(&[row("appearance", "{}"), row("player", "{ not json")])
        .await;

    assert!(result.is_err(), "json_valid check");
    assert_eq!(db.settings().rows().await.unwrap(), [], "all or nothing");
}

#[tokio::test]
async fn the_import_runs_once() {
    let (_dir, db) = fresh_db().await;
    let settings = db.settings();
    assert!(!settings.imported().await.unwrap(), "fresh install");

    let first = settings
        .import_once(&[row("appearance", "1")])
        .await
        .unwrap();
    let second = settings
        .import_once(&[row("appearance", "2")])
        .await
        .unwrap();

    assert!(first && !second, "only the first import stores anything");
    assert!(settings.imported().await.unwrap());
    assert_eq!(settings.rows().await.unwrap(), [row("appearance", "1")]);
}

#[tokio::test]
async fn the_import_never_overwrites_existing_rows() {
    let (_dir, db) = fresh_db().await;
    let settings = db.settings();
    settings.save(&[row("appearance", "1")]).await.unwrap();

    let imported = settings
        .import_once(&[row("appearance", "2")])
        .await
        .unwrap();

    assert!(!imported, "rows already exist");
    assert!(
        settings.imported().await.unwrap(),
        "and the import is marked done"
    );
    assert_eq!(settings.rows().await.unwrap(), [row("appearance", "1")]);
}

#[tokio::test]
async fn a_failed_import_leaves_it_to_run_again() {
    let (_dir, db) = fresh_db().await;

    let result = db
        .settings()
        .import_once(&[row("appearance", "{ bad")])
        .await;

    assert!(result.is_err(), "json_valid check");
    assert!(
        !db.settings().imported().await.unwrap(),
        "rolled back with the rows"
    );
}

/// Test helpers; inside `cfg(test)` so the workspace's test-only lint
/// allowances apply.
#[cfg(test)]
mod support {
    use db::Db;

    pub(crate) async fn fresh_db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        (dir, db)
    }

    pub(crate) fn row(tab: &str, json: &str) -> (String, String) {
        (tab.to_owned(), json.to_owned())
    }
}
