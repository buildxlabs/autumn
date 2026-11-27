//! Typed settings, their defaults and validation, and the app's OS paths.
//!
//! See ARCHITECTURE §2 and §6.

mod auto_select;
mod error;
mod paths;
mod settings;

pub use auto_select::{
    AutoSelectProfile, AutoSelectSettings, Codec, DEFAULT_PROFILE_NAME, Preference, Source,
};
pub use error::SettingsError;
pub use paths::AppPaths;
pub use settings::{
    AppearanceSettings, DEFAULT_MATCH_THRESHOLD, DEFAULT_MPCHC_HOST, DEFAULT_MPCHC_PORT,
    GeneralSettings, LibrarySettings, PlaybackKind, PlaybackSettings, PlayerChoice, Settings,
    TorrentSettings, WATCHED_THRESHOLD_RANGE, validate_torrent_folder,
};
