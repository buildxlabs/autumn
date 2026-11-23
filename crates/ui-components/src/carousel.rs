//! Arrow paging and Alt+wheel mapping for media carousels (#56).
//!
//! Drag-to-scroll and ordinary wheel paging come from `gpui-component`'s
//! carousel. This module owns the two input mappings Autumn adds on top:
//! arrow buttons page by one item, and Alt+wheel turns a vertical wheel
//! gesture into a horizontal scroll.

use gpui_kit::{
    App, Entity, Pixels, Point, ScrollEvent, component::carousel::CarouselState, point, px,
};

/// Which arrow the user pressed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "an arrow has exactly two directions and callers match both"
)]
pub enum PageDirection {
    /// The previous item.
    Previous,
    /// The next item.
    Next,
}

/// Page a carousel with an arrow button. Returns `false` when there is no
/// item in that direction, so the arrow can be shown as disabled. A looping
/// carousel wraps instead, so both arrows always report a move.
#[must_use]
pub fn page(state: &Entity<CarouselState>, direction: PageDirection, cx: &mut App) -> bool {
    state.update(cx, |state, cx| match direction {
        PageDirection::Previous => state.select_previous(cx),
        PageDirection::Next => state.select_next(cx),
    })
}

/// Map an Alt+wheel event to a horizontal scroll delta: the vertical wheel
/// movement becomes `x`. A scroll without Alt, or without vertical movement,
/// is left to the carousel. Pass `window.line_height()` so line deltas are
/// the right size; the caller applies the returned delta to the carousel.
#[must_use]
pub fn alt_wheel_delta(event: &ScrollEvent, line_height: Pixels) -> Option<Point<Pixels>> {
    if !event.modifiers.alt {
        return None;
    }
    let delta = event.delta.pixel_delta(line_height);
    if delta.y == px(0.) {
        return None;
    }
    Some(point(delta.y, px(0.)))
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext as _, Modifiers, ScrollDelta, TestAppContext};

    use super::*;

    #[gpui_kit::test]
    fn arrow_buttons_page_one_item_and_stop_at_the_ends(cx: &mut TestAppContext) {
        let state = cx.update(|cx| cx.new(|_| CarouselState::new(3)));
        let selected =
            |cx: &mut TestAppContext| state.read_with(cx, |state, _| state.selected_index());

        assert_eq!(selected(cx), Some(0), "the first item starts selected");
        assert!(cx.update(|cx| page(&state, PageDirection::Next, cx)));
        assert_eq!(selected(cx), Some(1));
        assert!(cx.update(|cx| page(&state, PageDirection::Next, cx)));
        assert_eq!(selected(cx), Some(2));
        assert!(!cx.update(|cx| page(&state, PageDirection::Next, cx)));
        assert_eq!(
            selected(cx),
            Some(2),
            "the last arrow cannot page past the end"
        );

        assert!(cx.update(|cx| page(&state, PageDirection::Previous, cx)));
        assert_eq!(selected(cx), Some(1));
        assert!(cx.update(|cx| page(&state, PageDirection::Previous, cx)));
        assert_eq!(selected(cx), Some(0));
        assert!(!cx.update(|cx| page(&state, PageDirection::Previous, cx)));
        assert_eq!(
            selected(cx),
            Some(0),
            "the first arrow cannot page past the start"
        );

        let empty = cx.update(|cx| cx.new(|_| CarouselState::new(0)));
        assert!(!cx.update(|cx| page(&empty, PageDirection::Next, cx)));
        assert_eq!(
            empty.read_with(cx, |state, _| state.selected_index()),
            None,
            "an empty carousel has no selection"
        );
    }

    #[test]
    fn alt_wheel_maps_vertical_movement_to_horizontal_scroll() {
        let line_height = px(20.);
        let alt_scroll = ScrollEvent {
            delta: ScrollDelta::Lines(point(0., -3.)),
            modifiers: Modifiers::alt(),
            ..Default::default()
        };
        assert_eq!(
            alt_wheel_delta(&alt_scroll, line_height),
            Some(point(px(-60.), px(0.))),
            "three Alt+wheel lines scroll three line-heights to the left"
        );

        let plain_scroll = ScrollEvent {
            delta: ScrollDelta::Lines(point(0., -3.)),
            ..Default::default()
        };
        assert_eq!(alt_wheel_delta(&plain_scroll, line_height), None);

        let already_horizontal = ScrollEvent {
            delta: ScrollDelta::Pixels(point(px(12.), px(0.))),
            modifiers: Modifiers::alt(),
            ..Default::default()
        };
        assert_eq!(alt_wheel_delta(&already_horizontal, line_height), None);
    }
}
