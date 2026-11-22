use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

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
}

impl Settings {
    /// Loads settings from `path`. A missing file means first run: the
    /// defaults are written there and returned.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Parse`] when the file is not valid settings JSON (the
    /// file is left untouched), or [`SettingsError::Io`] when it cannot be
    /// read or the defaults cannot be written.
    pub fn load_or_init(path: &Path) -> Result<Self, SettingsError> {
        match fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes).map_err(|source| SettingsError::Parse {
                path: path.to_path_buf(),
                source,
            }),
            Err(err) if err.kind() == io::ErrorKind::NotFound => {
                let defaults = Self::default();
                defaults.save(path)?;
                Ok(defaults)
            }
            Err(err) => Err(SettingsError::io(path)(err)),
        }
    }

    /// Writes the settings to `path` atomically: a sibling temporary file is
    /// written and synced, then renamed over `path`.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Io`] when the temporary file cannot be written or
    /// renamed, or [`SettingsError::Serialize`] if the settings cannot be
    /// encoded.
    pub fn save(&self, path: &Path) -> Result<(), SettingsError> {
        let json = serde_json::to_vec_pretty(self).map_err(SettingsError::Serialize)?;
        let tmp = temp_path(path);
        write_synced(&tmp, &json).map_err(SettingsError::io(&tmp))?;
        fs::rename(&tmp, path).map_err(SettingsError::io(path))
    }
}

/// `settings.json` → `settings.json.tmp`, in the same directory so the rename
/// stays on one filesystem.
fn temp_path(path: &Path) -> PathBuf {
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    path.with_file_name(name)
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let file = fs::File::create(path)?;
    io::Write::write_all(&mut &file, bytes)?;
    file.sync_all()
}
