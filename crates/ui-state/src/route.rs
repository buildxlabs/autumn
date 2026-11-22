//! The places the route view can show. See ARCHITECTURE §5.

/// One screen the route view can show.
///
/// ARCHITECTURE §5 lists the full set; a variant is added when its screen
/// exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "screens match every route, so a new route must not compile until it has a screen"
)]
pub enum Route {
    /// The landing screen.
    Home,
    /// Browse the catalog.
    Discover,
    /// The airing schedule.
    Schedule,
    /// Manga library and sources.
    Manga,
    /// Installed and available extensions.
    Extensions,
    /// Offline snapshots and sync.
    Sync,
    /// App settings.
    Settings,
}

impl Route {
    /// Every route, in rail order.
    pub const ALL: [Self; 7] = [
        Self::Home,
        Self::Discover,
        Self::Schedule,
        Self::Manga,
        Self::Extensions,
        Self::Sync,
        Self::Settings,
    ];

    /// A stable name for storing the route in settings. Never change one:
    /// saved settings refer to it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Home => "home",
            Self::Discover => "discover",
            Self::Schedule => "schedule",
            Self::Manga => "manga",
            Self::Extensions => "extensions",
            Self::Sync => "sync",
            Self::Settings => "settings",
        }
    }

    /// The route stored under `key`, if any.
    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|route| route.key() == key)
    }
}

#[cfg(test)]
mod tests {
    use super::Route;

    #[test]
    fn every_route_round_trips_through_its_key() {
        for route in Route::ALL {
            assert_eq!(Route::from_key(route.key()), Some(route), "{route:?}");
        }
        assert_eq!(Route::from_key("retired"), None, "unknown keys are ignored");
    }
}
