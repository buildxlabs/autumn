//! Browser-style navigation history with scroll restore. See ARCHITECTURE §5.

use gpui_kit::{Pixels, Point};

/// How many scroll offsets are kept; leaving one more route drops the
/// least recently saved.
const OFFSET_CAP: usize = 50;

/// The route on screen, the back and forward stacks, and a scroll offset
/// saved per route.
///
/// Routes are compared by value, so routes of one kind with different data
/// (two entries, two searches) keep separate offsets. Every move takes the
/// scroll offset of the route being left and returns the offset to restore
/// on the route being entered: the one saved when it was last left, or the
/// top for a route not yet visited or whose offset was dropped.
#[derive(Debug, Clone)]
pub struct RouterState<R> {
    current: R,
    back: Vec<R>,
    forward: Vec<R>,
    /// Saved offsets, least recently saved first; at most [`OFFSET_CAP`].
    offsets: Vec<(R, Point<Pixels>)>,
}

impl<R: Clone + Eq> RouterState<R> {
    /// History that starts at `route`, with nothing to go back or forward to.
    pub fn new(route: R) -> Self {
        Self {
            current: route,
            back: Vec::new(),
            forward: Vec::new(),
            offsets: Vec::new(),
        }
    }

    /// The route on screen.
    pub fn current(&self) -> &R {
        &self.current
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
    pub fn push(&mut self, route: R, left_at: Point<Pixels>) -> Option<Point<Pixels>> {
        if route == self.current {
            return None;
        }
        self.forward.clear();
        let (left, restore) = self.enter(route, left_at);
        self.back.push(left);
        Some(restore)
    }

    /// Returns to the previous route, or `None` when there is none.
    pub fn back(&mut self, left_at: Point<Pixels>) -> Option<Point<Pixels>> {
        let route = self.back.pop()?;
        let (left, restore) = self.enter(route, left_at);
        self.forward.push(left);
        Some(restore)
    }

    /// Redoes the last [`Self::back`], or `None` when there is none.
    pub fn forward(&mut self, left_at: Point<Pixels>) -> Option<Point<Pixels>> {
        let route = self.forward.pop()?;
        let (left, restore) = self.enter(route, left_at);
        self.back.push(left);
        Some(restore)
    }

    /// Switches to `route`, saving the current route's offset. Returns the
    /// route that was left and the offset to restore on `route`.
    ///
    /// The offset is read before the save, so saving never evicts the
    /// offset of the route being entered.
    fn enter(&mut self, route: R, left_at: Point<Pixels>) -> (R, Point<Pixels>) {
        let restore = self.offset(&route);
        let left = std::mem::replace(&mut self.current, route);
        self.save(left.clone(), left_at);
        (left, restore)
    }

    fn save(&mut self, route: R, offset: Point<Pixels>) {
        self.offsets.retain(|(saved, _)| *saved != route);
        self.offsets.push((route, offset));
        if self.offsets.len() > OFFSET_CAP {
            self.offsets.remove(0);
        }
    }

    fn offset(&self, route: &R) -> Point<Pixels> {
        self.offsets
            .iter()
            .find(|(saved, _)| saved == route)
            .map_or_else(Point::default, |&(_, offset)| offset)
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

        assert_eq!(*router.current(), Route::Home, "starts where told");
        assert!(!router.can_go_back(), "nothing behind");
        assert!(!router.can_go_forward(), "nothing ahead");
    }

    #[test]
    fn push_then_back_then_forward() {
        let mut router = RouterState::new(Route::Home);

        router.push(Route::Discover, at(0.));
        router.push(Route::Schedule, at(0.));
        assert_eq!(*router.current(), Route::Schedule, "after two pushes");
        assert!(router.can_go_back(), "home and discover are behind");

        router.back(at(0.));
        assert_eq!(*router.current(), Route::Discover, "one back");
        assert!(router.can_go_forward(), "schedule is ahead");

        router.back(at(0.));
        assert_eq!(*router.current(), Route::Home, "two back");
        assert!(!router.can_go_back(), "history start");
        assert_eq!(router.back(at(0.)), None, "back at the start does nothing");
        assert_eq!(*router.current(), Route::Home, "still home");

        router.forward(at(0.));
        router.forward(at(0.));
        assert_eq!(*router.current(), Route::Schedule, "two forward");
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
            *router.current(),
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

    /// Two routes of one kind with different data, like `Entry { id: 1 }`
    /// and `Entry { id: 2 }`.
    #[derive(Debug, Clone, PartialEq, Eq)]
    enum Page {
        List,
        Entry(u32),
    }

    #[test]
    fn routes_with_different_data_keep_separate_offsets() {
        let mut router = RouterState::new(Page::List);

        router.push(Page::Entry(1), at(0.));
        router.push(Page::Entry(2), at(400.));
        assert_eq!(
            router.back(at(250.)),
            Some(at(400.)),
            "entry 1 keeps its own offset"
        );
        assert_eq!(
            router.forward(at(0.)),
            Some(at(250.)),
            "entry 2 keeps its own offset"
        );
    }

    #[test]
    fn only_the_most_recent_offsets_are_kept() {
        let mut router = RouterState::new(Page::Entry(0));

        // Leave entries 0..=50: 51 saves, one over the cap.
        for id in 1..=51 {
            router.push(Page::Entry(id), at(10.));
        }

        assert_eq!(
            router.push(Page::Entry(0), at(10.)),
            Some(at(0.)),
            "the oldest offset was dropped, so entry 0 opens at the top"
        );
        assert_eq!(
            router.push(Page::Entry(2), at(10.)),
            Some(at(10.)),
            "newer offsets are kept"
        );
        assert_eq!(
            router.push(Page::Entry(1), at(10.)),
            Some(at(0.)),
            "saving entry 51 on the way to entry 0 dropped entry 1 in turn"
        );
    }
}
