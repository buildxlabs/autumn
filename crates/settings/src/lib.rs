//! Typed settings, their defaults and validation, and the app's OS paths.
//!
//! See ARCHITECTURE §2 and §6.

mod error;
mod paths;
mod settings;

pub use error::SettingsError;
pub use paths::AppPaths;
pub use settings::{AppearanceSettings, LibrarySettings, Settings};
