//! Presentational widgets that render plain view models.
//!
//! See ARCHITECTURE §2 and §5.

pub mod fetch_status;
mod image_cache;
mod rail;

pub use image_cache::{LruImageCache, POSTER_IMAGE_CAPACITY};
pub use rail::{ACCOUNT_ID, RAIL_EXPANDED_WIDTH, RAIL_WIDTH, Rail, RailExpansion, RailItem};
