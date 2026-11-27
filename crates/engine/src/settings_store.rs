//! Settings live in the database. The Phase 0 `settings.json` is imported
//! once and then retired (ROADMAP Phase 1, decision #17). Moved from
//! `autumn` when the engine crate landed (#58).

use std::{
    fs,
    path::{Path, PathBuf},
};

use db::Db;
use settings::{Settings, SettingsError};

use crate::EngineError;

/// What `settings.json` is renamed to once imported, so the import never runs
/// again.
const IMPORTED_SUFFIX: &str = "imported";
/// What a `settings.json` that can't be parsed is renamed to. It is kept for
/// the user to recover by hand, and never imported.
const CORRUPT_SUFFIX: &str = "corrupt";

/// Imports `settings_file` if it still exists, then loads the settings from
/// the database. A fresh install has no rows and gets the defaults.
///
/// A corrupt file or bad row is logged, and the affected settings take their
/// defaults.
pub(crate) async fn load(db: &Db, settings_file: &Path) -> Result<Settings, EngineError> {
    import_once(db, settings_file).await?;

    let rows = db.settings().rows().await?;
    let (settings, errors) = Settings::from_rows(rows);
    for err in errors {
        log_error(&err, "a settings row is invalid; its tab uses the defaults");
    }
    Ok(settings)
}

/// Imports `file` unless the database says the import already ran. The rows
/// and the "imported" mark commit together, so renaming the file afterwards is
/// only tidying: if it fails, the next start skips the file anyway. A start
/// with no file marks the import done, so a file that appears later is never
/// imported over the database.
async fn import_once(db: &Db, file: &Path) -> Result<(), EngineError> {
    if db.settings().imported().await? {
        if file.exists() {
            retire(file, IMPORTED_SUFFIX);
        }
        return Ok(());
    }
    match Settings::read_file(file) {
        Ok(None) => {
            // A fresh install: there is nothing to import, now or later.
            db.settings().import_once(&[]).await?;
            Ok(())
        }
        Ok(Some(settings)) => {
            let imported = db.settings().import_once(&settings.to_rows()?).await?;
            if imported {
                tracing::info!(file = %file.display(), "imported settings.json into the database");
            } else {
                tracing::info!(file = %file.display(), "kept the newer database settings over settings.json");
            }
            retire(file, IMPORTED_SUFFIX);
            Ok(())
        }
        Err(err @ SettingsError::Parse { .. }) => {
            log_error(
                &err,
                "settings.json is corrupt; not imported, defaults apply",
            );
            // Marked done with no rows, so a corrupt file is never retried.
            db.settings().import_once(&[]).await?;
            retire(file, CORRUPT_SUFFIX);
            Ok(())
        }
        Err(err) => {
            // Unreadable (permissions, I/O): try again next start. Rows the
            // user saves meanwhile win over the file then (`import_once`).
            log_error(
                &err,
                "settings.json can't be read; not imported, defaults apply",
            );
            Ok(())
        }
    }
}

/// Renames `settings.json` to `settings.json.<suffix>`. A failure is only
/// logged: the database already records that the import ran.
fn retire(file: &Path, suffix: &str) {
    let retired = with_suffix(file, suffix);
    if let Err(err) = fs::rename(file, &retired) {
        tracing::warn!(
            error = %err,
            from = %file.display(),
            to = %retired.display(),
            "could not retire settings.json; it will be ignored"
        );
    }
}

fn with_suffix(file: &Path, suffix: &str) -> PathBuf {
    let mut name = file.as_os_str().to_os_string();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}

fn log_error(err: &SettingsError, message: &str) {
    let mut chain = err.to_string();
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        chain.push_str(": ");
        chain.push_str(&cause.to_string());
        source = cause.source();
    }
    tracing::warn!(error = %chain, "{message}");
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::{Arc, Mutex},
    };

    use db::Db;
    use settings::{AppearanceSettings, LibrarySettings, PlaybackSettings, Settings};
    use tempfile::TempDir;

    use super::load;

    const PINNED: &str = r#"{"appearance":{"sidebar_click_pin":true}}"#;

    async fn setup() -> (TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("autumn.db")).await.unwrap();
        (dir, db)
    }

    fn pinned() -> Settings {
        Settings {
            appearance: AppearanceSettings {
                sidebar_click_pin: true,
                ..AppearanceSettings::default()
            },
            library: LibrarySettings::default(),
            playback: PlaybackSettings::default(),
            general: settings::GeneralSettings::default(),
            ..Settings::default()
        }
    }

    #[tokio::test]
    async fn an_existing_file_imports_once_and_is_retired() {
        let (dir, db) = setup().await;
        let file = dir.path().join("settings.json");
        fs::write(&file, PINNED).unwrap();

        let loaded = load(&db, &file).await.unwrap();

        assert_eq!(
            loaded,
            pinned(),
            "the click-pin setting survives the upgrade"
        );
        assert!(!file.exists(), "settings.json is retired");
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json.imported")).unwrap(),
            PINNED,
            "kept under its retired name"
        );
        assert_eq!(
            db.settings().rows().await.unwrap().len(),
            7,
            "seven rows: appearance, library, playback, general, auto_select,\
             torrent and network tabs"
        );
    }

    #[tokio::test]
    async fn a_second_start_does_not_re_import() {
        let (dir, db) = setup().await;
        let file = dir.path().join("settings.json");
        fs::write(&file, PINNED).unwrap();
        load(&db, &file).await.unwrap();
        // The user turns click-pin off; the change is saved to the database.
        db.settings()
            .save(&Settings::default().to_rows().unwrap())
            .await
            .unwrap();

        let loaded = load(&db, &file).await.unwrap();

        assert_eq!(
            loaded,
            Settings::default(),
            "the database wins, not the old file"
        );
    }

    #[tokio::test]
    async fn a_file_left_behind_after_the_import_is_not_re_imported() {
        let (dir, db) = setup().await;
        let file = dir.path().join("settings.json");
        fs::write(&file, PINNED).unwrap();
        load(&db, &file).await.unwrap();
        db.settings()
            .save(&Settings::default().to_rows().unwrap())
            .await
            .unwrap();
        // As if the rename had failed, or a crash hit right after the commit.
        fs::write(&file, PINNED).unwrap();

        let loaded = load(&db, &file).await.unwrap();

        assert_eq!(loaded, Settings::default(), "the database wins");
        assert!(!file.exists(), "the leftover is retired");
    }

    #[tokio::test]
    async fn a_file_appearing_after_a_fresh_install_is_not_imported() {
        let (dir, db) = setup().await;
        let file = dir.path().join("settings.json");
        load(&db, &file).await.unwrap();

        fs::write(&file, PINNED).unwrap();
        let loaded = load(&db, &file).await.unwrap();

        assert_eq!(
            loaded,
            Settings::default(),
            "the fresh install's settings win"
        );
    }

    #[tokio::test]
    async fn a_file_unreadable_at_first_never_overwrites_newer_rows() {
        let (dir, db) = setup().await;
        let file = dir.path().join("settings.json");
        // A directory where the file should be: reading it is an I/O error.
        fs::create_dir(&file).unwrap();
        assert_eq!(load(&db, &file).await.unwrap(), Settings::default());
        // The user changes a setting while the file can't be read.
        db.settings()
            .save(&pinned().to_rows().unwrap())
            .await
            .unwrap();

        fs::remove_dir(&file).unwrap();
        fs::write(&file, r#"{"appearance":{"sidebar_click_pin":false}}"#).unwrap();
        let loaded = load(&db, &file).await.unwrap();

        assert_eq!(loaded, pinned(), "the newer database rows win");
    }

    #[tokio::test]
    async fn with_no_file_defaults_apply() {
        let (dir, db) = setup().await;

        let loaded = load(&db, &dir.path().join("settings.json")).await.unwrap();

        assert_eq!(loaded, Settings::default());
        assert_eq!(db.settings().rows().await.unwrap(), [], "nothing written");
    }

    #[tokio::test]
    async fn a_corrupt_file_is_logged_and_defaults_are_used() {
        let (dir, db) = setup().await;
        let file = dir.path().join("settings.json");
        fs::write(&file, "{ not json").unwrap();
        let logs = Logs::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .finish();
        let _logging = tracing::subscriber::set_default(subscriber);

        let loaded = load(&db, &file).await.unwrap();

        assert_eq!(loaded, Settings::default());
        assert!(
            logs.text().contains("settings.json is corrupt"),
            "{}",
            logs.text()
        );
        assert_eq!(
            fs::read_to_string(dir.path().join("settings.json.corrupt")).unwrap(),
            "{ not json",
            "kept for the user, out of the import's way"
        );
        assert!(!file.exists(), "not retried next start");
    }

    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl Logs {
        fn text(&self) -> String {
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
