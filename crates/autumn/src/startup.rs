//! What runs before the window opens: app dirs, logging and settings
//! (ROADMAP Phase 0).

use anyhow::Context as _;
use settings::{AppPaths, Settings};
use tracing_appender::non_blocking::WorkerGuard;

/// Everything startup produced. Keep it alive for the whole run: it owns the
/// log writer's flush guard.
#[derive(Debug)]
#[expect(
    dead_code,
    reason = "the shell reads paths and settings once it exists (#39, #41)"
)]
pub(crate) struct Started {
    /// The resolved and created app directories.
    pub(crate) paths: AppPaths,
    /// The settings loaded from disk, or the defaults.
    pub(crate) settings: Settings,
    _log_guard: WorkerGuard,
}

/// Creates the app dirs, installs logging, then loads `settings.json`.
///
/// A corrupt settings file is logged and the defaults are used for this run;
/// the file is left in place for the user.
pub(crate) fn run() -> anyhow::Result<Started> {
    let paths = AppPaths::resolve()?;
    paths
        .create_dirs()
        .context("could not create the app directories")?;
    let log_guard = crate::logging::init(&paths.log_dir())?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %paths.config_dir().display(),
        data = %paths.data_dir().display(),
        cache = %paths.cache_dir().display(),
        "starting Autumn"
    );

    let settings = Settings::load_or_init(&paths.settings_file()).unwrap_or_else(|err| {
        tracing::error!(error = %anyhow::Error::from(err), "using default settings");
        Settings::default()
    });
    tracing::debug!(?settings, "settings loaded");

    Ok(Started {
        paths,
        settings,
        _log_guard: log_guard,
    })
}
