//! One view per route: home, entry, discover, search, settings and the rest.
//!
//! See ARCHITECTURE §2 and §5.

mod lists;
mod lists_source;
mod palette;
mod palette_search;
mod placeholder;
mod search_catalog;
mod shell;

pub use shell::{AppShell, PosterCacheAction, RailMode, RailPrefs, SYSTEM_ROUTES};
