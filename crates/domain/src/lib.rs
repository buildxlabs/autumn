//! Domain types shared by every Autumn crate: ids, media models and events.
//!
//! No I/O lives here. See ARCHITECTURE §2.

mod fuzzy_date;
mod list_entry;
mod media;
mod score;

pub use fuzzy_date::{FuzzyDate, FuzzyDateError};
pub use list_entry::{AdvancedScores, ListEntry, ListStatus};
pub use media::{ExternalId, MediaId, MediaKind};
pub use score::{Score, ScoreError};

/// An event the engine publishes for the UI to apply to its stores.
///
/// Events travel on the bounded channel described in ARCHITECTURE §4.
/// Each engine module adds its own variants as it lands.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AppEvent {
    /// Stand-in until the first engine module publishes a real event.
    Placeholder,
}
