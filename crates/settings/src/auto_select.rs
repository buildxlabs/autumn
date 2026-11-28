//! Auto-select profiles (#181 Q8, Q10; ADR 0007).
//!
//! `search` filters and ranks indexer results with the effective profile:
//! the per-media override when one is set, else the global default, else the
//! shipped `Default`.

use std::collections::{BTreeMap, HashSet};

use serde::{Deserialize, Serialize};

use crate::SettingsError;

/// The name of the shipped profile.
pub const DEFAULT_PROFILE_NAME: &str = "Default";

/// How a profile treats a yes/no property of a release.
///
/// `Prefer` and `Avoid` only rank; `Only` and `Never` are hard filters.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the profile editor and the ranking handle every preference"
)]
#[serde(rename_all = "snake_case")]
pub enum Preference {
    /// No effect on filtering or ranking.
    #[default]
    Neutral,
    /// Rank releases with the property first.
    Prefer,
    /// Rank releases with the property last.
    Avoid,
    /// Drop releases without the property.
    Only,
    /// Drop releases with the property.
    Never,
}

/// A video codec a profile can prefer, mirroring what `release-parser` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the profile editor lists every codec the parser recognizes"
)]
#[serde(rename_all = "snake_case")]
pub enum Codec {
    /// H.264 / AVC.
    H264,
    /// H.265 / HEVC.
    H265,
    /// AV1.
    Av1,
    /// Xvid.
    Xvid,
    /// VP9.
    Vp9,
    /// MPEG-2.
    Mpeg2,
}

/// A video source a profile can prefer, mirroring what `release-parser` reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the profile editor lists every source the parser recognizes"
)]
#[serde(rename_all = "snake_case")]
pub enum Source {
    /// Blu-ray or remux.
    BluRay,
    /// Blu-ray re-encode.
    BdRip,
    /// Streaming download.
    WebDl,
    /// Streaming re-encode.
    WebRip,
    /// DVD.
    Dvd,
    /// DVD re-encode.
    DvdRip,
    /// TV capture.
    Tv,
}

/// A named set of filters and ordered ranking criteria (glossary
/// **Auto-select profile**).
///
/// Ordered lists rank earlier entries higher; an empty list is no preference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoSelectProfile {
    /// The profile's unique name.
    pub name: String,
    /// Vertical resolutions in lines, best first.
    pub resolutions: Vec<u32>,
    /// Release groups, best first, matched case-insensitively.
    pub release_groups: Vec<String>,
    /// Video codecs, best first.
    pub codecs: Vec<Codec>,
    /// Releases with fewer seeders are dropped; unknown counts as zero.
    pub min_seeders: u32,
    /// Releases smaller than this many bytes, or of unknown size, are dropped.
    pub min_size: Option<u64>,
    /// Releases larger than this many bytes, or of unknown size, are dropped.
    pub max_size: Option<u64>,
    /// Releases whose name contains any of these, case-insensitively, are dropped.
    pub excluded_terms: Vec<String>,
    /// Subtitle or audio language tags (`ENG`, `VOSTFR`), best first, matched
    /// as words in the release name.
    pub preferred_languages: Vec<String>,
    /// Video sources, best first.
    pub sources: Vec<Source>,
    /// Dual or multi audio.
    pub multiple_audio: Preference,
    /// A batch rather than a single episode.
    pub batch: Preference,
    /// Multiple subtitle languages.
    pub multiple_subs: Preference,
    /// Drop releases whose codec is not in [`Self::codecs`].
    pub require_codec: bool,
    /// Drop releases whose source is not in [`Self::sources`].
    pub require_source: bool,
    /// Drop releases naming none of [`Self::preferred_languages`].
    pub require_language: bool,
}

impl Default for AutoSelectProfile {
    fn default() -> Self {
        Self {
            name: DEFAULT_PROFILE_NAME.to_owned(),
            resolutions: vec![1080, 720, 480],
            release_groups: Vec::new(),
            codecs: Vec::new(),
            min_seeders: 0,
            min_size: None,
            max_size: None,
            excluded_terms: Vec::new(),
            preferred_languages: Vec::new(),
            sources: Vec::new(),
            multiple_audio: Preference::Neutral,
            batch: Preference::Neutral,
            multiple_subs: Preference::Neutral,
            require_codec: false,
            require_source: false,
            require_language: false,
        }
    }
}

/// The auto-select profiles, the global default and per-media overrides.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AutoSelectSettings {
    /// Every named profile, the shipped `Default` included.
    pub profiles: Vec<AutoSelectProfile>,
    /// The name of the profile used when a media has no override.
    pub default_profile: String,
    /// Per-media overrides: a raw `MediaId` to a profile name.
    pub media_profiles: BTreeMap<i64, String>,
}

impl Default for AutoSelectSettings {
    fn default() -> Self {
        Self {
            profiles: vec![AutoSelectProfile::default()],
            default_profile: DEFAULT_PROFILE_NAME.to_owned(),
            media_profiles: BTreeMap::new(),
        }
    }
}

impl AutoSelectSettings {
    /// The profile auto-select uses for `media` (a raw `MediaId`).
    ///
    /// The media's override, else the global default, else the shipped
    /// `Default` when a name no longer refers to a profile.
    #[must_use]
    pub fn effective(&self, media: Option<i64>) -> AutoSelectProfile {
        media
            .and_then(|media| self.media_profiles.get(&media))
            .and_then(|name| self.find(name))
            .or_else(|| self.find(&self.default_profile))
            .cloned()
            .unwrap_or_default()
    }

    fn find(&self, name: &str) -> Option<&AutoSelectProfile> {
        self.profiles.iter().find(|profile| profile.name == name)
    }

    fn find_mut(&mut self, name: &str) -> Option<&mut AutoSelectProfile> {
        self.profiles
            .iter_mut()
            .find(|profile| profile.name == name)
    }

    /// Checks a profile name is usable for add/duplicate/rename.
    ///
    /// `taken` is the profile being renamed (its current name doesn't clash
    /// with itself). Names are unique case-insensitively; blank is rejected.
    fn check_name(&self, name: &str, taken: Option<&str>) -> Result<(), SettingsError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(SettingsError::Validation {
                field: "auto_select.profiles.name",
                message: "every profile needs its own name",
            });
        }
        let clashes = self.profiles.iter().any(|profile| {
            profile.name.eq_ignore_ascii_case(trimmed)
                && taken.is_none_or(|taken| !profile.name.eq_ignore_ascii_case(taken))
        });
        if clashes {
            return Err(SettingsError::Validation {
                field: "auto_select.profiles.name",
                message: "every profile needs its own name",
            });
        }
        Ok(())
    }

    /// Adds a new profile with shipped defaults under `name`.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Validation`] when `name` is blank or already taken
    /// (case-insensitively).
    pub fn add_profile(&mut self, name: &str) -> Result<(), SettingsError> {
        self.check_name(name, None)?;
        let profile = AutoSelectProfile {
            name: name.trim().to_owned(),
            ..AutoSelectProfile::default()
        };
        self.profiles.push(profile);
        Ok(())
    }

    /// Copies profile `name` under a fresh unique name and returns that name.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Validation`] when no profile is named `name`.
    pub fn duplicate_profile(&mut self, name: &str) -> Result<String, SettingsError> {
        let source = self.find(name).cloned().ok_or(SettingsError::Validation {
            field: "auto_select.profiles.name",
            message: "no profile with that name",
        })?;
        let mut copy_name = format!("{} copy", source.name);
        let mut counter = 2;
        while self
            .profiles
            .iter()
            .any(|p| p.name.eq_ignore_ascii_case(&copy_name))
        {
            copy_name = format!("{} copy {counter}", source.name);
            counter += 1;
        }
        let mut copy = source;
        copy_name.clone_into(&mut copy.name);
        self.profiles.push(copy);
        Ok(copy_name)
    }

    /// Renames profile `name` to `new_name`, updating the global default and
    /// per-media overrides that referenced it.
    ///
    /// The shipped `Default` cannot be renamed. Renaming to your own name is
    /// a no-op.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Validation`] when `name` names no profile, `new_name`
    /// is blank or taken, or `name` is the shipped `Default`.
    pub fn rename_profile(&mut self, name: &str, new_name: &str) -> Result<(), SettingsError> {
        if name == DEFAULT_PROFILE_NAME {
            return Err(SettingsError::Validation {
                field: "auto_select.profiles.name",
                message: "the shipped Default profile cannot be renamed",
            });
        }
        if self.find(name).is_none() {
            return Err(SettingsError::Validation {
                field: "auto_select.profiles.name",
                message: "no profile with that name",
            });
        }
        let new_name = new_name.trim();
        self.check_name(new_name, Some(name))?;
        let profile = self.find_mut(name).ok_or(SettingsError::Validation {
            field: "auto_select.profiles.name",
            message: "no profile with that name",
        })?;
        let old_name = profile.name.clone();
        new_name.clone_into(&mut profile.name);
        if self.default_profile == old_name {
            new_name.clone_into(&mut self.default_profile);
        }
        for target in self.media_profiles.values_mut() {
            if *target == old_name {
                new_name.clone_into(target);
            }
        }
        Ok(())
    }

    /// Removes profile `name`.
    ///
    /// The shipped `Default` cannot be removed. References fall back cleanly:
    /// the global default resets to `Default`, and per-media overrides naming
    /// `name` are dropped, so [`Self::effective`] keeps working.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Validation`] when no profile is named `name`, or when
    /// `name` is the shipped `Default`.
    pub fn delete_profile(&mut self, name: &str) -> Result<(), SettingsError> {
        if name == DEFAULT_PROFILE_NAME {
            return Err(SettingsError::Validation {
                field: "auto_select.profiles.name",
                message: "the shipped Default profile cannot be deleted",
            });
        }
        if self.find(name).is_none() {
            return Err(SettingsError::Validation {
                field: "auto_select.profiles.name",
                message: "no profile with that name",
            });
        }
        self.profiles.retain(|profile| profile.name != name);
        if self.default_profile == name {
            DEFAULT_PROFILE_NAME.clone_into(&mut self.default_profile);
        }
        self.media_profiles.retain(|_, target| target != name);
        Ok(())
    }

    /// Restores the shipped `Default` profile to its shipped values.
    ///
    /// The global default and per-media overrides are untouched; `Default`
    /// itself is re-added if it is missing.
    pub fn reset_default_profile(&mut self) {
        let shipped = AutoSelectProfile::default();
        if let Some(profile) = self.find_mut(DEFAULT_PROFILE_NAME) {
            *profile = shipped;
        } else {
            self.profiles.push(shipped);
        }
    }

    /// Makes `name` the global default profile.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Validation`] when no profile is named `name`.
    pub fn set_global_default(&mut self, name: &str) -> Result<(), SettingsError> {
        if self.find(name).is_none() {
            return Err(SettingsError::Validation {
                field: "auto_select.default_profile",
                message: "choose an existing profile",
            });
        }
        name.clone_into(&mut self.default_profile);
        Ok(())
    }

    /// Checks the values the profile editor must keep valid.
    ///
    /// # Errors
    ///
    /// [`SettingsError::Validation`] when a profile name is blank or repeated
    /// (case-insensitively), a size range is inverted, or the default or an
    /// override names no profile.
    pub fn validate(&self) -> Result<(), SettingsError> {
        let mut names = HashSet::new();
        for profile in &self.profiles {
            let name = profile.name.trim();
            if name.is_empty() || !names.insert(name.to_lowercase()) {
                return Err(SettingsError::Validation {
                    field: "auto_select.profiles.name",
                    message: "every profile needs its own name",
                });
            }
            if let (Some(min), Some(max)) = (profile.min_size, profile.max_size)
                && min > max
            {
                return Err(SettingsError::Validation {
                    field: "auto_select.profiles.max_size",
                    message: "the maximum size must not be below the minimum",
                });
            }
        }
        if self.find(&self.default_profile).is_none() {
            return Err(SettingsError::Validation {
                field: "auto_select.default_profile",
                message: "choose an existing profile",
            });
        }
        if self
            .media_profiles
            .values()
            .any(|name| self.find(name).is_none())
        {
            return Err(SettingsError::Validation {
                field: "auto_select.media_profiles",
                message: "a media override names a profile that does not exist",
            });
        }
        Ok(())
    }
}

/// Moves the element at `from` to `to`; out-of-range or no-op moves change nothing.
///
/// Reorders ordered profile lists (reorderable chips).
pub fn move_item<T>(items: &mut Vec<T>, from: usize, to: usize) {
    if from >= items.len() || to >= items.len() || from == to {
        return;
    }
    let item = items.remove(from);
    items.insert(to, item);
}
