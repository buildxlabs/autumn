use std::{fs, io, path::Path};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::SettingsError;

/// Every user setting, one field per settings tab.
///
/// Unknown fields are ignored and missing ones take their default, so files
/// written by older or newer versions still load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The Appearance tab.
    pub appearance: AppearanceSettings,
}

/// The Appearance tab.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppearanceSettings {
    /// When on, clicking the sidebar rail pins it open and the expanded rail
    /// pushes the content aside instead of drawing over it on hover.
    pub sidebar_click_pin: bool,
    /// Sidebar items the user moved into the "More" menu, by route key.
    /// Unknown keys are ignored, so a removed route never breaks loading.
    pub sidebar_unpinned: Vec<String>,
}

impl Settings {
    /// Reads the Phase 0 `settings.json` at `path`, for the one-time import
    /// into the database. `None` when there is no file.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Parse`] when the file is not valid settings JSON, or
    /// [`SettingsError::Io`] when it exists but cannot be read.
    pub fn read_file(path: &Path) -> Result<Option<Self>, SettingsError> {
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(err) => return Err(SettingsError::io(path)(err)),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|source| SettingsError::Parse {
                path: path.to_path_buf(),
                source,
            })
    }

    /// One `(tab, JSON)` row per settings tab, the shape `db` stores.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Serialize`] if a tab cannot be encoded.
    pub fn to_rows(&self) -> Result<Vec<(String, String)>, SettingsError> {
        let Value::Object(tabs) = serde_json::to_value(self).map_err(SettingsError::Serialize)?
        else {
            return Ok(Vec::new());
        };
        tabs.into_iter()
            .map(|(tab, value)| {
                let json = serde_json::to_string(&value).map_err(SettingsError::Serialize)?;
                Ok((tab, json))
            })
            .collect()
    }

    /// Rebuilds settings from `(tab, JSON)` rows. Missing tabs and fields take
    /// their defaults and unknown tabs are ignored. A row that does not parse
    /// leaves its tab at the defaults and is returned as an error, so one bad
    /// tab never costs the others.
    pub fn from_rows(
        rows: impl IntoIterator<Item = (String, String)>,
    ) -> (Self, Vec<SettingsError>) {
        let mut tabs = Map::new();
        let mut errors = Vec::new();
        for (tab, json) in rows {
            let parsed = serde_json::from_str::<Value>(&json).and_then(|value| {
                // Check the tab alone, so a bad row only resets its own tab.
                let mut alone = Map::new();
                alone.insert(tab.clone(), value.clone());
                serde_json::from_value::<Self>(Value::Object(alone)).map(|_| value)
            });
            match parsed {
                Ok(value) => {
                    tabs.insert(tab, value);
                }
                Err(source) => errors.push(SettingsError::Row { tab, source }),
            }
        }
        // Every tab kept above deserialized on its own, so together they do too.
        let settings = serde_json::from_value(Value::Object(tabs)).unwrap_or_default();
        (settings, errors)
    }
}
