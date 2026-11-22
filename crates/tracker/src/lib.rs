//! Tracking platforms: the [`Platform`] trait and the account-free [`Local`]
//! platform.
//!
//! Exactly one platform is active at a time, and each keeps its own list
//! entries per media (ADR 0004). See ARCHITECTURE §6.

mod collection;
mod error;
mod local;
mod platform;

pub use collection::{Collection, List, ListName};
pub use error::TrackerError;
pub use local::Local;
pub use platform::Platform;
