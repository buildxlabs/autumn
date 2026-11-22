//! What runs before the window opens: app dirs, logging and settings
//! (ROADMAP Phase 0).

use anyhow::Context as _;
use settings::{AppPaths, Settings};

/// Everything startup produced.
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
    crate::logging::init(&paths.log_dir())?;
    tracing::info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %paths.config_dir().display(),
        data = %paths.data_dir().display(),
        cache = %paths.cache_dir().display(),
        "starting Autumn"
    );

    let settings = Settings::load_or_init(&paths.settings_file()).unwrap_or_else(|err| {
        let err = anyhow::Error::from(err);
        tracing::error!(error = %format_args!("{err:#}"), "using default settings");
        Settings::default()
    });
    // Settings hold no secrets (those live in the keychain), so the whole
    // struct may be logged. Revisit if a secret-adjacent field is added.
    tracing::debug!(?settings, "settings loaded");

    Ok(Started { paths, settings })
}
