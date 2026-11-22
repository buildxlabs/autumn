//! Presentational widgets that render plain view models.
//!
//! See ARCHITECTURE §2 and §5.

pub mod fetch_status;
mod rail;

pub use rail::{RAIL_EXPANDED_WIDTH, RAIL_WIDTH, Rail, RailExpansion, RailItem};
