//! One view per route: home, entry, discover, search, settings and the rest.
//!
//! See ARCHITECTURE §2 and §5.

mod palette;
mod placeholder;
mod shell;

pub use shell::{AppShell, PosterCacheAction, RailMode, RailPrefs, SYSTEM_ROUTES};
