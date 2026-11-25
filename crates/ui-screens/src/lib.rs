//! One view per route: home, entry, discover, search, settings and the rest.
//!
//! See ARCHITECTURE §2 and §5.

mod discover;
mod entry;
mod home;
mod lists;
mod lists_source;
mod palette;
mod palette_search;
mod placeholder;
mod search;
mod search_catalog;
mod shell;

pub use discover::{
    DiscoverLoader, DiscoverStore, DiscoverUpdate, DiscoverView, RealDiscoverCatalog,
};

pub use shell::{
    AppShell, LibraryServices, LibrarySettings, PlaybackServices, PlaybackSettings,
    PlaybackTestError, PlayerOption, PosterCacheAction, RailMode, RailPrefs, SYSTEM_ROUTES,
};
