use std::{
    fs, io,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::SettingsError;

/// The default title-similarity a file needs to match a candidate, in percent (#127).
pub const DEFAULT_MATCH_THRESHOLD: u8 = 50;
/// The watched-at threshold bounds, as a percentage of a file's duration.
pub const WATCHED_THRESHOLD_RANGE: std::ops::RangeInclusive<u8> = 50..=100;
/// The default MPC-HC web-interface host, used when the field is left empty.
pub const DEFAULT_MPCHC_HOST: &str = "127.0.0.1";
/// The default MPC-HC web-interface port (#119).
pub const DEFAULT_MPCHC_PORT: u16 = 13579;

/// Every user setting, one field per settings tab.
///
/// Unknown fields are ignored and missing ones take their default, so files
/// written by older or newer versions still load.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// The Appearance tab.
    pub appearance: AppearanceSettings,
    /// The Playback tab.
    pub playback: PlaybackSettings,
    /// The Library tab.
    pub library: LibrarySettings,
    /// The General tab, including the first-run wizard flag.
    pub general: GeneralSettings,
}

/// The General tab. A new install and a Phase 1 install both start with
/// `onboarding_completed` false, so each sees the wizard once (#138).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// Whether the first-run wizard has been completed.
    pub onboarding_completed: bool,
}

impl GeneralSettings {
    /// Marks the wizard done. The other tabs keep their current values.
    pub fn complete(&mut self) {
        self.onboarding_completed = true;
    }

    /// Reopens the wizard from Settings → General.
    pub fn reopen(&mut self) {
        self.onboarding_completed = false;
    }
}

/// The Library tab.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LibrarySettings {
    /// When on, library hydration looks up filler episode lists on
    /// animefillerlist.com. On by default (OD-3).
    pub fetch_filler_episodes: bool,
    /// Folders scanned for local files.
    pub folders: Vec<PathBuf>,
    /// Whether a watcher runs scans when the library folders change.
    pub auto_scan: bool,
    /// The lowest title similarity that wins a match, in percent.
    pub matching_threshold: u8,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            fetch_filler_episodes: true,
            folders: Vec::new(),
            auto_scan: true,
            matching_threshold: DEFAULT_MATCH_THRESHOLD,
        }
    }
}

impl LibrarySettings {
    /// Checks the values the Library tab must keep in range.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::Validation`] when the matching threshold
    /// is above 100 percent.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if self.matching_threshold > 100 {
            return Err(SettingsError::Validation {
                field: "library.matching_threshold",
                message: "must be at most 100 percent",
            });
        }
        Ok(())
    }
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

/// The player families Autumn can launch. Kept in `settings` so this crate
/// stays self-contained; the `player` crate owns the launch behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Playback tab handles every player family"
)]
#[serde(rename_all = "snake_case")]
pub enum PlaybackKind {
    /// mpv, over JSON IPC.
    Mpv,
    /// IINA on macOS, over its embedded mpv's JSON IPC.
    Iina,
    /// VLC, over its HTTP interface.
    Vlc,
    /// MPC-HC, over its web interface.
    MpcHc,
}

impl PlaybackKind {
    /// The name shown in settings.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mpv => "mpv",
            Self::Iina => "IINA",
            Self::Vlc => "VLC",
            Self::MpcHc => "MPC-HC",
        }
    }
}

/// Which player to use. Detection resolves an [`PlayerChoice::Auto`] or
/// [`PlayerChoice::Installed`] to a command at play time, so a player that was
/// upgraded or moved since the setting was saved is still found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Playback tab handles every choice"
)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerChoice {
    /// Use the preferred installed player for this platform.
    Auto,
    /// Use the first installed player of this kind.
    Installed {
        /// The player family to launch.
        kind: PlaybackKind,
    },
    /// Use an explicit executable chosen with the file picker.
    Custom {
        /// The player family the executable belongs to.
        kind: PlaybackKind,
        /// The executable path.
        path: PathBuf,
    },
}

/// The Settings → Playback tab.
///
/// Serde defaults let an older or newer row load without losing the other
/// tabs. [`Self::validate`] checks the values the UI must keep in range.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "these are independent toggles, not modes of one state machine"
)]
#[serde(default)]
pub struct PlaybackSettings {
    /// Which player to launch.
    pub player: PlayerChoice,
    /// The host of MPC-HC's web interface.
    pub mpchc_host: String,
    /// The port of MPC-HC's web interface.
    pub mpchc_port: u16,
    /// Percentage of a file's duration at which an episode counts as watched.
    pub watched_threshold: u8,
    /// Update the active platform's progress when an episode counts as watched.
    pub auto_sync_progress: bool,
    /// Seek to the saved position after the player reports media ready.
    pub resume: bool,
    /// Offer the next episode when one finishes.
    pub auto_next: bool,
    /// Seconds to wait before starting the next episode.
    pub auto_next_seconds: u8,
    /// Skip opening and ending chapters automatically.
    pub skip_op_ed: bool,
    /// Chapter titles treated as an opening, matched case-insensitively.
    pub opening_patterns: Vec<String>,
    /// Chapter titles treated as an ending, matched case-insensitively.
    pub ending_patterns: Vec<String>,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            player: PlayerChoice::Auto,
            mpchc_host: DEFAULT_MPCHC_HOST.to_owned(),
            mpchc_port: DEFAULT_MPCHC_PORT,
            watched_threshold: 80,
            auto_sync_progress: true,
            resume: true,
            auto_next: true,
            auto_next_seconds: 5,
            skip_op_ed: true,
            opening_patterns: vec!["^OP$".to_owned(), "Opening".to_owned(), "Intro".to_owned()],
            ending_patterns: vec!["^ED$".to_owned(), "Ending".to_owned(), "Credits".to_owned()],
        }
    }
}

impl PlaybackSettings {
    /// Checks the values the Playback tab must keep in range.
    ///
    /// # Errors
    ///
    /// Returns [`SettingsError::Validation`] when the watched threshold is
    /// outside 50–100, the countdown is zero, an MPC-HC field is unusable, or
    /// a custom player does not point at a file.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if !WATCHED_THRESHOLD_RANGE.contains(&self.watched_threshold) {
            return Err(SettingsError::Validation {
                field: "playback.watched_threshold",
                message: "must be between 50 and 100 percent",
            });
        }
        if self.auto_next_seconds == 0 {
            return Err(SettingsError::Validation {
                field: "playback.auto_next_seconds",
                message: "the countdown must be at least one second",
            });
        }
        let mpchc_selected = match &self.player {
            PlayerChoice::Installed { kind } | PlayerChoice::Custom { kind, .. } => {
                *kind == PlaybackKind::MpcHc
            }
            PlayerChoice::Auto => false,
        };
        if mpchc_selected {
            if self.mpchc_host.trim().is_empty() {
                return Err(SettingsError::Validation {
                    field: "playback.mpchc_host",
                    message: "host cannot be empty",
                });
            }
            if self.mpchc_port == 0 {
                return Err(SettingsError::Validation {
                    field: "playback.mpchc_port",
                    message: "port must not be zero",
                });
            }
        }
        if let PlayerChoice::Custom { path, .. } = &self.player
            && !path.is_file()
        {
            return Err(SettingsError::Validation {
                field: "playback.player.path",
                message: "choose a player executable that exists",
            });
        }
        Ok(())
    }
}

impl Settings {
    /// Marks the first-run wizard done, so the next launch goes straight to
    /// Home. The other tabs are left as they are.
    pub fn complete_onboarding(&mut self) {
        self.general.onboarding_completed = true;
    }

    /// Reopens the wizard from Settings → General, keeping the current
    /// choices as the wizard's starting values.
    pub fn reopen_onboarding(&mut self) {
        self.general.onboarding_completed = false;
    }

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
