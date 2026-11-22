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
}
