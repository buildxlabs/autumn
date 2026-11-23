//! Tracking platforms: the [`Platform`] trait and the account-free [`Local`]
//! platform.
//!
//! Exactly one platform is active at a time, and each keeps its own list
//! entries per media (ADR 0004). See ARCHITECTURE §6.

mod active;
mod anilist;
mod collection;
mod error;
mod local;
mod platform;
mod transfer;

pub use active::{ActiveKind, ActivePlatform};
pub use anilist::AniListPlatform;
pub use collection::{Collection, List, ListName};
pub use error::PlatformError;
pub use local::Local;
pub use platform::Platform;
pub use transfer::{collection_entries, plan_transfer, run_transfer};
