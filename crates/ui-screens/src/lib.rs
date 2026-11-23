//! One view per route: home, entry, discover, search, settings and the rest.
//!
//! See ARCHITECTURE §2 and §5.

mod discover;
mod lists;
mod lists_source;
mod palette;
mod palette_search;
mod placeholder;
mod shell;

pub use discover::{
    AIRING_TTL, CATALOG_TTL, DiscoverCatalog, DiscoverKey, DiscoverLoader, DiscoverSection,
    DiscoverStore, DiscoverView, RealDiscoverCatalog, Section, SectionEntries, SectionLoad,
};

pub use shell::{AppShell, PosterCacheAction, RailMode, RailPrefs, SYSTEM_ROUTES};
