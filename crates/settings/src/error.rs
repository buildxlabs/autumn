use std::{io, path::PathBuf};

/// Why paths could not be resolved or settings could not be read or written.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SettingsError {
    /// The OS reported no home directory, so there is nowhere to put app files.
    #[error("no home directory found for the app's config, data and cache dirs")]
    NoHomeDir,
    /// A file or directory could not be read, written, created or renamed.
    #[error("I/O error on {}", path.display())]
    Io {
        /// The file or directory being accessed.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: io::Error,
    },
    /// The Phase 0 `settings.json` exists but is not valid settings JSON.
    #[error("{} is not valid settings JSON", path.display())]
    Parse {
        /// The settings file.
        path: PathBuf,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// A database settings row is not valid JSON for its tab.
    #[error("settings row {tab:?} is not valid")]
    Row {
        /// The tab the row belongs to.
        tab: String,
        /// The underlying error.
        #[source]
        source: serde_json::Error,
    },
    /// The settings could not be encoded as JSON.
    #[error("could not encode the settings as JSON")]
    Serialize(#[source] serde_json::Error),
    /// A settings value is outside the range or form the app accepts.
    #[error("settings field {field:?} is invalid: {message}")]
    Validation {
        /// The field that failed validation.
        field: &'static str,
        /// Why the value was rejected.
        message: &'static str,
    },
}

impl SettingsError {
    pub(crate) fn io(path: impl Into<PathBuf>) -> impl FnOnce(io::Error) -> Self {
        let path = path.into();
        move |source| Self::Io { path, source }
    }
}
