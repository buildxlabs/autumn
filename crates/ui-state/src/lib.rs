//! The Tokio bridge, UI stores, router, actions and the engine event pump.
//!
//! See ARCHITECTURE §2, §4 and §5.

pub mod account;
pub mod actions;
mod badges;
pub mod bridge;
pub mod events;
pub mod fetch_status;
pub mod lists;
mod route;
mod router;
mod scroll;
pub mod search;

pub use account::{AccountCommand, AccountStore};
pub use badges::RailBadges;
pub use lists::{
    ListCard, ListSort, ListSource, ListSourceError, ListTab, ListsSession, QuickEdit, STATUS_ORDER,
};
pub use route::{Route, SettingsTab};
pub use router::RouterState;
pub use scroll::{ContentLoaded, ScrollRestore};
pub use search::{
    Catalog, CatalogError, MediaPreview, RestoredPage, SearchFilters, SearchFormat, SearchPage,
    SearchSeason, SearchSession, SearchSort, SearchStatus,
};
