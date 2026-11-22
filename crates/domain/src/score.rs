use serde::{Deserialize, Serialize};

/// The largest raw score.
const MAX_RAW: u8 = 100;

/// A list entry's score, stored raw on AniList's 0–100 scale. 0 means
/// unscored (decision #16).
///
/// The display format is a separate choice; `POINT_10_DECIMAL` is the default.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(try_from = "u8", into = "u8")]
pub struct Score(u8);

/// Why a value is not a valid [`Score`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ScoreError {
    /// The value is outside 0–100 (or 0.0–10.0 in `POINT_10_DECIMAL`), or not
    /// a number.
    #[error("score out of range")]
    OutOfRange,
}

impl Score {
    /// No score given.
    pub const UNSCORED: Self = Self(0);

    /// A raw 0–100 score, 0 meaning unscored.
    ///
    /// # Errors
    ///
    /// [`ScoreError::OutOfRange`] above 100.
    pub const fn new(raw: u8) -> Result<Self, ScoreError> {
        if raw > MAX_RAW {
            return Err(ScoreError::OutOfRange);
        }
        Ok(Self(raw))
    }

    /// The stored 0–100 value.
    pub const fn raw(self) -> u8 {
        self.0
    }

    /// Whether the user gave a score.
    pub const fn is_scored(self) -> bool {
        self.0 != 0
    }

    /// The score on AniList's `POINT_10_DECIMAL` scale (85 → 8.5), or `None`
    /// when unscored.
    pub fn to_point_10_decimal(self) -> Option<f32> {
        self.is_scored().then(|| f32::from(self.0) / 10.0)
    }

    /// Reads a `POINT_10_DECIMAL` score (8.5 → 85), rounded to one decimal.
    /// 0.0 means unscored, so anything below 0.05 rounds to unscored.
    ///
    /// # Errors
    ///
    /// [`ScoreError::OutOfRange`] outside 0.0–10.0, or for NaN.
    pub fn from_point_10_decimal(shown: f32) -> Result<Self, ScoreError> {
        if !(0.0..=10.0).contains(&shown) {
            return Err(ScoreError::OutOfRange);
        }
        let raw = (shown * 10.0).round();
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "raw is a whole number in 0.0..=100.0, checked above"
        )]
        let raw = raw as u8;
        Self::new(raw)
    }
}

impl TryFrom<u8> for Score {
    type Error = ScoreError;

    fn try_from(raw: u8) -> Result<Self, Self::Error> {
        Self::new(raw)
    }
}

impl From<Score> for u8 {
    fn from(score: Score) -> Self {
        score.0
    }
}
