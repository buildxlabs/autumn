//! What runs before the window opens: app dirs, logging, the database and
//! settings (ROADMAP Phases 0 and 1).

use anyhow::Context as _;
use db::Db;
use settings::{AppPaths, Settings};
use tokio::runtime::Runtime;

/// Everything startup produced.
#[derive(Debug)]
#[expect(dead_code, reason = "the engine reads the paths once it exists (#39)")]
pub(crate) struct Started {
    /// The resolved and created app directories.
    pub(crate) paths: AppPaths,
    /// The open database. Its pool runs on `runtime`.
    pub(crate) db: Db,
    /// The settings from the database, or the defaults.
    pub(crate) settings: Settings,
}

/// Creates the app dirs, installs logging, opens the database, imports the
/// Phase 0 `settings.json` once, then loads the settings.
///
/// A corrupt settings file is logged and the defaults are used.
pub(crate) fn run(runtime: &Runtime) -> anyhow::Result<Started> {
    let paths = AppPaths::resolve()?;
    paths
        .create_dirs()
        .context("could not create the app directories")?;
    crate::logging::init(&paths.log_dir())?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %paths.config_dir().display(),
        data = %paths.data_dir().display(),
        cache = %paths.cache_dir().display(),
        "starting Autumn"
    );

    let (db, settings) = runtime.block_on(async {
        let db = Db::open(&paths.database_file())
            .await
            .context("could not open the database")?;
        let settings = crate::settings_store::load(&db, &paths.settings_file()).await?;
        anyhow::Ok((db, settings))
    })?;
    // Settings hold no secrets (those live in the keychain), so the whole
    // struct may be logged. Revisit if a secret-adjacent field is added.
    tracing::debug!(?settings, "settings loaded");

    Ok(Started {
        paths,
        db,
        settings,
    })
}
