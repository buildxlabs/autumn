//! The Tokio bridge, UI stores, router, actions and the engine event pump.
//!
//! See ARCHITECTURE §2, §4 and §5.

pub mod actions;
mod badges;
pub mod bridge;
pub mod events;
mod route;
mod router;
mod scroll;

pub use badges::RailBadges;
pub use route::Route;
pub use router::RouterState;
pub use scroll::{ContentLoaded, ScrollRestore};
