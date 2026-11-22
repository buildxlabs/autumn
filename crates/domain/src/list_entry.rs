use std::time::SystemTime;

use serde::{Deserialize, Serialize};

use crate::{FuzzyDate, MediaId, Score};

/// A user's tracking record for one media on one platform, with AniList's
/// full `MediaList` field set (decision #16), so a list transfer loses
/// nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListEntry {
    /// The media this entry tracks.
    pub media: MediaId,
    /// Where the media sits in the user's list.
    pub status: ListStatus,
    /// The overall score, raw 0–100.
    pub score: Score,
    /// Episodes watched, or chapters read.
    pub progress: u32,
    /// Volumes read; manga only, 0 for anime.
    pub progress_volumes: u32,
    /// How many times the user has rewatched or reread it.
    pub repeat: u32,
    /// When the user started.
    pub started_at: FuzzyDate,
    /// When the user finished.
    pub completed_at: FuzzyDate,
    /// Free-text notes.
    pub notes: String,
    /// Hidden from other users on AniList. No effect on the Local platform;
    /// kept so a list transfer carries it over.
    pub private: bool,
    /// The user's priority for planned media; 0 means none.
    pub priority: u32,
    /// Per-category scores.
    pub advanced_scores: AdvancedScores,
    /// Names of the custom lists this entry is in.
    pub custom_lists: Vec<String>,
    /// Shown only in its custom lists, not under its status.
    pub hidden_from_status_lists: bool,
    /// When the entry was created.
    pub created_at: SystemTime,
    /// When the entry last changed.
    pub updated_at: SystemTime,
}

/// Where a media sits in a user's list, with AniList's six values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[non_exhaustive]
pub enum ListStatus {
    /// Watching or reading now.
    Current,
    /// Planned.
    Planning,
    /// Finished.
    Completed,
    /// Given up.
    Dropped,
    /// On hold.
    Paused,
    /// Rewatching or rereading.
    Repeating,
}

/// AniList's five default advanced-scoring categories, each raw 0–100 with
/// 0 meaning unscored. Whether they are shown is a setting, not part of the
/// entry (decision #16).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct AdvancedScores {
    /// Story.
    pub story: Score,
    /// Characters.
    pub characters: Score,
    /// Visuals.
    pub visuals: Score,
    /// Audio.
    pub audio: Score,
    /// Enjoyment.
    pub enjoyment: Score,
}
