//! The places the route view can show. See ARCHITECTURE §5.

use domain::ListStatus;

use crate::lists::ListTab;
use crate::search::SearchFilters;

/// A page in Settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the settings screen handles every tab"
)]
pub enum SettingsTab {
    /// First-run setup and the flag that controls it.
    General,
    /// Appearance and rail interaction.
    Appearance,
    /// AniList connection, avatar and token expiry.
    Account,
    /// Player choice and playback options.
    Playback,
    /// Library folders, matching and manual scans.
    Library,
    /// Disk cache usage and controls.
    Storage,
    /// Torrent networking, proxies and indexer mirrors (#210 / ADR-0010).
    Network,
    /// Auto-select profiles (#228); torrent folder and cache stay on Storage.
    Torrents,
    /// Data-source attribution and licences.
    About,
}

/// A tab of the Anime Entry page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the Entry screen renders every tab"
)]
pub enum EntryTab {
    /// The details, relations and list editor, as in Phase 1.
    Overview,
    /// The local files as episode cards (#132).
    Library,
    /// The episode list with torrent streaming and one-click auto-select
    /// (#220, #182 Q1).
    Torrents,
}

impl EntryTab {
    /// The tabs shown, in order (#182 Q1). `Library` only with local files;
    /// `Torrents` whenever torrent streaming is enabled. The tab bar always
    /// renders once `torrents_enabled`.
    #[must_use]
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "two independent tab-bar conditions"
    )]
    pub fn visible(has_files: bool, torrents_enabled: bool) -> Vec<Self> {
        let mut tabs = vec![Self::Overview];
        if has_files {
            tabs.push(Self::Library);
        }
        if torrents_enabled {
            tabs.push(Self::Torrents);
        }
        tabs
    }

    /// The tab actually shown (#182 Q1). The default is unchanged: `Library`
    /// with files, else `Overview`. `Torrents` needs torrent streaming
    /// enabled; `Library` needs files. An unknown combination falls back to
    /// `Overview`.
    #[must_use]
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "two independent tab conditions"
    )]
    pub fn resolve(tab: Option<Self>, has_files: bool, torrents_enabled: bool) -> Self {
        match tab {
            Some(Self::Torrents) if torrents_enabled => Self::Torrents,
            Some(Self::Library) | None if has_files => Self::Library,
            _ => Self::Overview,
        }
    }
}

/// One screen the route view can show.
///
/// ARCHITECTURE §5 lists the full set; a variant is added when its screen
/// exists.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "screens match every route, so a new route must not compile until it has a screen"
)]
pub enum Route {
    /// The first-run wizard, shown before Home until setup is completed.
    Onboarding,
    /// The landing screen.
    Home,
    /// The Anime Entry page of one anime (#58).
    Entry {
        /// The anime's AniList id.
        id: u32,
        /// The tab the user picked; `None` opens Library when the media has
        /// local files and Overview otherwise.
        tab: Option<EntryTab>,
    },
    /// Browse the catalog.
    Discover,
    /// The active platform's list entries in one tab.
    Lists {
        /// The selected status or custom list.
        status: ListTab,
    },
    /// Advanced Search with the committed filters.
    Search(SearchFilters),
    /// The airing schedule.
    Schedule,
    /// The live list of torrents in Autumn's own session (#207).
    Torrents,
    /// The audit log of recent library scans (#134); not a rail item.
    ScanSummaries,
    /// Installed and available extensions.
    Extensions,
    /// Offline snapshots and sync.
    Sync,
    /// App settings.
    Settings {
        /// The selected page.
        tab: SettingsTab,
    },
}

impl Route {
    /// The default destination for each rail item, in rail order.
    pub const ALL: [Self; 7] = [
        Self::Home,
        Self::Discover,
        Self::Torrents,
        Self::Schedule,
        Self::Extensions,
        Self::Sync,
        Self::Settings {
            tab: SettingsTab::Appearance,
        },
    ];

    /// Every route that has a stable settings key, including screens that
    /// are not rail items. [`Self::ALL`] stays the rail's default set.
    pub const ALL_KEYED: [Self; 10] = [
        Self::Home,
        Self::Discover,
        Self::Lists {
            status: ListTab::Status(ListStatus::Current),
        },
        Self::Search(SearchFilters::EMPTY),
        Self::Schedule,
        Self::Torrents,
        Self::ScanSummaries,
        Self::Extensions,
        Self::Sync,
        Self::Settings {
            tab: SettingsTab::Appearance,
        },
    ];

    /// A stable name for storing the route in settings. Never change one:
    /// saved settings refer to it.
    pub fn key(&self) -> &'static str {
        match self {
            Self::Onboarding => "onboarding",
            Self::Home => "home",
            Self::Entry { .. } => "entry",
            Self::Discover => "discover",
            Self::Lists { .. } => "lists",
            Self::Search(_) => "search",
            Self::Schedule => "schedule",
            Self::Torrents => "torrents",
            Self::ScanSummaries => "scan_summaries",
            Self::Extensions => "extensions",
            Self::Sync => "sync",
            Self::Settings { .. } => "settings",
        }
    }

    /// The route stored under `key`, if any.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL_KEYED
            .iter()
            .find(|route| route.key() == key)
            .cloned()
    }
}

#[cfg(test)]
mod tests {
    use crate::lists::ListTab;

    use super::{EntryTab, Route};

    #[test]
    fn every_route_round_trips_through_its_key() {
        for route in Route::ALL_KEYED {
            assert_eq!(
                Route::from_key(route.key()).as_ref(),
                Some(&route),
                "{route:?}"
            );
        }
        assert_eq!(Route::from_key("retired"), None, "unknown keys are ignored");
        assert_eq!(Route::from_key("manga"), None, "a retired key is ignored");
    }

    #[test]
    fn torrents_tab_is_a_deep_linkable_entry_tab() {
        let route = Route::Entry {
            id: 100,
            tab: Some(EntryTab::Torrents),
        };
        assert_eq!(route.key(), "entry");
        // A fresh visit never picks Torrents by default; an explicit pick holds.
        assert_eq!(
            EntryTab::resolve(Some(EntryTab::Torrents), false, true),
            EntryTab::Torrents
        );
        assert_eq!(
            EntryTab::resolve(Some(EntryTab::Torrents), true, true),
            EntryTab::Torrents
        );
    }

    #[test]
    fn tab_bar_shows_torrents_and_library_only_when_allowed() {
        // Overview · Library (only with files) · Torrents (#182 Q1).
        assert_eq!(
            EntryTab::visible(false, true),
            vec![EntryTab::Overview, EntryTab::Torrents]
        );
        assert_eq!(
            EntryTab::visible(true, true),
            vec![EntryTab::Overview, EntryTab::Library, EntryTab::Torrents]
        );
        assert_eq!(
            EntryTab::visible(true, false),
            vec![EntryTab::Overview, EntryTab::Library]
        );
        assert_eq!(EntryTab::visible(false, false), vec![EntryTab::Overview]);
    }

    #[test]
    fn the_default_tab_is_unchanged() {
        assert_eq!(EntryTab::resolve(None, true, true), EntryTab::Library);
        assert_eq!(EntryTab::resolve(None, false, true), EntryTab::Overview);
        // An explicit Torrents pick needs torrent streaming; Library needs files.
        assert_eq!(
            EntryTab::resolve(Some(EntryTab::Torrents), true, false),
            EntryTab::Overview
        );
        assert_eq!(
            EntryTab::resolve(Some(EntryTab::Library), false, true),
            EntryTab::Overview
        );
    }

    #[test]
    fn list_tabs_are_distinct_route_values() {
        let favourites = Route::Lists {
            status: ListTab::Custom("Favourites".to_owned()),
        };
        let watching = Route::Lists {
            status: ListTab::default(),
        };
        assert_eq!(favourites.key(), "lists", "the rail key ignores the tab");
        assert_ne!(favourites, watching, "each tab keeps its own scroll slot");
        assert_eq!(
            Route::from_key("lists"),
            Some(Route::Lists {
                status: ListTab::default()
            }),
            "a persisted Lists route restores its default tab"
        );
    }
}
