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
    pub const ALL: [Self; 6] = [
        Self::Home,
        Self::Discover,
        Self::Schedule,
        Self::Extensions,
        Self::Sync,
        Self::Settings {
            tab: SettingsTab::Appearance,
        },
    ];

    /// Every route that has a stable settings key, including screens that
    /// are not rail items. [`Self::ALL`] stays the rail's default set.
    pub const ALL_KEYED: [Self; 9] = [
        Self::Home,
        Self::Discover,
        Self::Lists {
            status: ListTab::Status(ListStatus::Current),
        },
        Self::Search(SearchFilters::EMPTY),
        Self::Schedule,
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

    use super::Route;

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
