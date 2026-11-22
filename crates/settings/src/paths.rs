use std::{
    fs,
    path::{Path, PathBuf},
};

use directories::ProjectDirs;

use crate::SettingsError;

/// The `ProjectDirs` qualifier, organization and application (decision #17).
///
/// They give `dev.buildxlabs.Autumn` on macOS, `autumn` on Linux and
/// `buildxlabs\Autumn` on Windows.
const PROJECT: (&str, &str, &str) = ("dev", "buildxlabs", "Autumn");

/// The app's config, data and cache directories (ARCHITECTURE §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    config: PathBuf,
    data: PathBuf,
    cache: PathBuf,
}

impl AppPaths {
    /// Uses the given directories as-is, without touching the disk.
    pub fn new(config: PathBuf, data: PathBuf, cache: PathBuf) -> Self {
        Self {
            config,
            data,
            cache,
        }
    }

    /// Resolves the OS-standard directories for Autumn. Linux honors the XDG
    /// variables. Nothing is created; call [`AppPaths::create_dirs`] for that.
    ///
    /// # Errors
    ///
    /// [`SettingsError::NoHomeDir`] when the OS reports no home directory.
    pub fn resolve() -> Result<Self, SettingsError> {
        let (qualifier, organization, application) = PROJECT;
        let dirs = ProjectDirs::from(qualifier, organization, application)
            .ok_or(SettingsError::NoHomeDir)?;
        Ok(Self::new(
            dirs.config_dir().to_path_buf(),
            dirs.data_dir().to_path_buf(),
            dirs.cache_dir().to_path_buf(),
        ))
    }

    /// Creates the config, data and cache directories if they are missing.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Io`] naming the directory that could not be created.
    pub fn create_dirs(&self) -> Result<(), SettingsError> {
        for dir in [&self.config, &self.data, &self.cache] {
            fs::create_dir_all(dir).map_err(SettingsError::io(dir))?;
        }
        Ok(())
    }

    /// Holds `settings.json` and nothing else.
    pub fn config_dir(&self) -> &Path {
        &self.config
    }

    /// Holds the database and `logs/`.
    pub fn data_dir(&self) -> &Path {
        &self.data
    }

    /// Holds disposable caches; always safe to delete.
    pub fn cache_dir(&self) -> &Path {
        &self.cache
    }

    /// `<config>/settings.json`.
    pub fn settings_file(&self) -> PathBuf {
        self.config.join("settings.json")
    }

    /// `<data>/logs/`, where the rolling log files go.
    pub fn log_dir(&self) -> PathBuf {
        self.data.join("logs")
    }
}
