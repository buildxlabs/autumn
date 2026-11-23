//! What runs before the window opens: app dirs, logging. The database,
//! settings and shared client open through the engine (#58).

use anyhow::Context as _;
use settings::AppPaths;

/// What startup produced: the app directories, created and logged.
#[derive(Debug)]
pub(crate) struct Startup {
    /// The resolved and created app directories.
    pub(crate) paths: AppPaths,
}

/// Creates the app dirs and installs logging.
///
/// A failure here is before any user-facing state exists, so it can only
/// print: logging may not be installed yet.
pub(crate) fn run() -> anyhow::Result<Startup> {
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
    Ok(Startup { paths })
}
