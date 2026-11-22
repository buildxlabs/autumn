//! Browser-style navigation history with scroll restore. See ARCHITECTURE §5.

use std::collections::HashMap;

use gpui_kit::{Pixels, Point};

use crate::Route;

/// The route on screen, the back and forward stacks, and a scroll offset
/// saved per route.
///
/// Every move takes the scroll offset of the route being left and returns
/// the offset to restore on the route being entered: the one saved when it
/// was last left, or the top for a route not yet visited.
#[derive(Debug, Clone)]
pub struct RouterState {
    current: Route,
    back: Vec<Route>,
    forward: Vec<Route>,
    offsets: HashMap<Route, Point<Pixels>>,
}

impl RouterState {
    /// History that starts at `route`, with nothing to go back or forward to.
    pub fn new(route: Route) -> Self {
        Self {
            current: route,
            back: Vec::new(),
            forward: Vec::new(),
            offsets: HashMap::new(),
        }
    }

    /// The route on screen.
    pub fn current(&self) -> Route {
        self.current
    }

    /// Whether [`Self::back`] has somewhere to go.
    pub fn can_go_back(&self) -> bool {
        !self.back.is_empty()
    }

    /// Whether [`Self::forward`] has somewhere to go.
    pub fn can_go_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    /// Opens `route`, clearing the forward stack.
    ///
    /// Returns `None`, and changes nothing, when `route` is already on screen.
    pub fn push(&mut self, route: Route, left_at: Point<Pixels>) -> Option<Point<Pixels>> {
        if route == self.current {
            return None;
        }
        self.forward.clear();
        let left = self.enter(route, left_at);
        self.back.push(left);
        Some(self.offset(route))
    }

    /// Returns to the previous route, or `None` when there is none.
    pub fn back(&mut self, left_at: Point<Pixels>) -> Option<Point<Pixels>> {
        let route = self.back.pop()?;
        let left = self.enter(route, left_at);
        self.forward.push(left);
        Some(self.offset(route))
    }

    /// Redoes the last [`Self::back`], or `None` when there is none.
    pub fn forward(&mut self, left_at: Point<Pixels>) -> Option<Point<Pixels>> {
        let route = self.forward.pop()?;
        let left = self.enter(route, left_at);
        self.back.push(left);
        Some(self.offset(route))
    }

    /// Saves the current route's offset, switches to `route`, and returns the
    /// route that was left.
    fn enter(&mut self, route: Route, left_at: Point<Pixels>) -> Route {
        self.offsets.insert(self.current, left_at);
        std::mem::replace(&mut self.current, route)
    }

    fn offset(&self, route: Route) -> Point<Pixels> {
        self.offsets.get(&route).copied().unwrap_or_default()
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{Pixels, Point, point, px};

    use super::RouterState;
    use crate::Route;

    /// The offset after scrolling `y` pixels down; `at(0.)` is the top.
    fn at(y: f32) -> Point<Pixels> {
        point(px(0.), px(-y))
    }

    #[test]
    fn starts_with_empty_history() {
        let router = RouterState::new(Route::Home);

        assert_eq!(router.current(), Route::Home, "starts where told");
        assert!(!router.can_go_back(), "nothing behind");
        assert!(!router.can_go_forward(), "nothing ahead");
    }

    #[test]
    fn push_then_back_then_forward() {
        let mut router = RouterState::new(Route::Home);

        router.push(Route::Discover, at(0.));
        router.push(Route::Schedule, at(0.));
        assert_eq!(router.current(), Route::Schedule, "after two pushes");
        assert!(router.can_go_back(), "home and discover are behind");

        router.back(at(0.));
        assert_eq!(router.current(), Route::Discover, "one back");
        assert!(router.can_go_forward(), "schedule is ahead");

        router.back(at(0.));
        assert_eq!(router.current(), Route::Home, "two back");
        assert!(!router.can_go_back(), "history start");
        assert_eq!(router.back(at(0.)), None, "back at the start does nothing");
        assert_eq!(router.current(), Route::Home, "still home");

        router.forward(at(0.));
        router.forward(at(0.));
        assert_eq!(router.current(), Route::Schedule, "two forward");
        assert_eq!(
            router.forward(at(0.)),
            None,
            "forward at the end does nothing"
        );
    }

    #[test]
    fn push_clears_the_forward_stack() {
        let mut router = RouterState::new(Route::Home);
        router.push(Route::Discover, at(0.));
        router.back(at(0.));
        assert!(router.can_go_forward(), "discover is ahead");

        router.push(Route::Manga, at(0.));

        assert!(!router.can_go_forward(), "a new push drops discover");
        router.back(at(0.));
        assert_eq!(
            router.current(),
            Route::Home,
            "back goes to home, not discover"
        );
    }

    #[test]
    fn pushing_the_current_route_is_ignored() {
        let mut router = RouterState::new(Route::Home);

        assert_eq!(router.push(Route::Home, at(100.)), None, "no navigation");
        assert!(!router.can_go_back(), "no history entry");
    }

    #[test]
    fn offsets_are_restored_per_route() {
        let mut router = RouterState::new(Route::Home);

        assert_eq!(
            router.push(Route::Discover, at(300.)),
            Some(at(0.)),
            "discover opens at the top"
        );
        assert_eq!(
            router.push(Route::Schedule, at(50.)),
            Some(at(0.)),
            "schedule opens at the top"
        );
        assert_eq!(
            router.back(at(20.)),
            Some(at(50.)),
            "discover comes back where it was left"
        );
        assert_eq!(
            router.back(at(60.)),
            Some(at(300.)),
            "home comes back where it was left"
        );
        assert_eq!(
            router.forward(at(0.)),
            Some(at(60.)),
            "discover keeps its newer offset"
        );
        assert_eq!(
            router.forward(at(0.)),
            Some(at(20.)),
            "schedule keeps its offset too"
        );
    }
}
