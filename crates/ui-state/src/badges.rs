//! Live counts shown as badges on sidebar items. See ARCHITECTURE §5.

use std::collections::HashMap;

use crate::Route;

/// A count per route, shown as a pill on that route's sidebar item.
///
/// Held as an entity, so the shell redraws when a count changes. Real counts
/// (missing episodes, active torrents, queue size, extension updates) come
/// with their features; until then [`RailBadges::placeholder`] fills it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RailBadges {
    counts: HashMap<Route, u32>,
}

impl RailBadges {
    /// Made-up counts, so the badges can be seen before their features exist.
    pub fn placeholder() -> Self {
        let mut badges = Self::default();
        badges.set(Route::Schedule, 3);
        badges.set(Route::Extensions, 1);
        badges
    }

    /// The badge for `route`; `None` when it has no count or the count is 0.
    pub fn count(&self, route: &Route) -> Option<u32> {
        self.counts.get(route).copied().filter(|&count| count > 0)
    }

    /// Sets the count for `route`; 0 hides its badge.
    pub fn set(&mut self, route: Route, count: u32) {
        self.counts.insert(route, count);
    }
}

#[cfg(test)]
mod tests {
    use super::RailBadges;
    use crate::Route;

    #[test]
    fn a_zero_count_shows_no_badge() {
        let mut badges = RailBadges::default();
        badges.set(Route::Schedule, 2);
        assert_eq!(badges.count(&Route::Schedule), Some(2));

        badges.set(Route::Schedule, 0);
        assert_eq!(badges.count(&Route::Schedule), None);
        assert_eq!(badges.count(&Route::Home), None, "never set");
    }
}
