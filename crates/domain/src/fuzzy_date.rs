use serde::{Deserialize, Serialize};

/// A date where any part may be unknown, as AniList's `FuzzyDate`: a start
/// date can be just a year, or a year and month.
///
/// Known parts are always valid: month 1–12, and a day that exists in that
/// month (February 29 needs a leap year, or an unknown year).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawFuzzyDate")]
pub struct FuzzyDate {
    year: Option<u16>,
    month: Option<u8>,
    day: Option<u8>,
}

/// Why parts do not form a [`FuzzyDate`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum FuzzyDateError {
    /// The month is not 1–12.
    #[error("month {0} is not 1-12")]
    Month(u8),
    /// The day does not exist in that month.
    #[error("day {0} does not exist in that month")]
    Day(u8),
}

impl FuzzyDate {
    /// No part known.
    pub const UNKNOWN: Self = Self {
        year: None,
        month: None,
        day: None,
    };

    /// Builds a date from whichever parts are known.
    ///
    /// # Errors
    ///
    /// [`FuzzyDateError`] naming the first impossible part.
    pub fn new(
        year: Option<u16>,
        month: Option<u8>,
        day: Option<u8>,
    ) -> Result<Self, FuzzyDateError> {
        if let Some(month) = month
            && !(1..=12).contains(&month)
        {
            return Err(FuzzyDateError::Month(month));
        }
        if let Some(day) = day
            && !(1..=last_day(year, month)).contains(&day)
        {
            return Err(FuzzyDateError::Day(day));
        }
        Ok(Self { year, month, day })
    }

    /// The year, if known.
    pub const fn year(self) -> Option<u16> {
        self.year
    }

    /// The month (1–12), if known.
    pub const fn month(self) -> Option<u8> {
        self.month
    }

    /// The day of the month, if known.
    pub const fn day(self) -> Option<u8> {
        self.day
    }

    /// Whether no part is known.
    pub const fn is_unknown(self) -> bool {
        self.year.is_none() && self.month.is_none() && self.day.is_none()
    }
}

/// The last day that can exist in `month` of `year`, taking the most
/// permissive answer for unknown parts.
fn last_day(year: Option<u16>, month: Option<u8>) -> u8 {
    match month {
        Some(4 | 6 | 9 | 11) => 30,
        Some(2) if year.is_some_and(|year| !is_leap(year)) => 28,
        Some(2) => 29,
        _ => 31,
    }
}

fn is_leap(year: u16) -> bool {
    year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
}

/// The unchecked serde shape, validated through [`FuzzyDate::new`].
#[derive(Deserialize)]
struct RawFuzzyDate {
    year: Option<u16>,
    month: Option<u8>,
    day: Option<u8>,
}

impl TryFrom<RawFuzzyDate> for FuzzyDate {
    type Error = FuzzyDateError;

    fn try_from(raw: RawFuzzyDate) -> Result<Self, Self::Error> {
        Self::new(raw.year, raw.month, raw.day)
    }
}
