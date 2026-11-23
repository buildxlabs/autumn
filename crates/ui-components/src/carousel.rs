//! A carousel of media cards, with arrow paging and Alt+wheel mapping (#56).
//!
//! Drag-to-scroll and ordinary wheel paging come from `gpui-component`'s
//! carousel. This module adds the arrows and maps Alt+wheel to horizontal
//! paging, on top of the plain input helpers.

use gpui_kit::{
    App, ElementId, Entity, InteractiveElement, IntoElement, ParentElement, Pixels, Point,
    RenderOnce, ScrollEvent, Styled, TestSupportExt, Window,
    component::{
        Disableable,
        button::Button,
        carousel::{Carousel, CarouselContent, CarouselItem, CarouselState},
    },
    div, image_cache, point, px,
};

use crate::{LruImageCache, MEDIA_CARD_HEIGHT, MediaCard, MediaCardView, POSTER_WIDTH};

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

/// A horizontally paging row of media cards. The state is owned by the
/// caller so paging survives redraws and a newer view can replace the items.
#[derive(IntoElement)]
pub struct MediaCarousel {
    /// Prefix for every child id, so a screen can show several carousels.
    prefix: String,
    state: Entity<CarouselState>,
    items: Vec<MediaCardView>,
    cache: Entity<LruImageCache>,
}

impl std::fmt::Debug for MediaCarousel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MediaCarousel")
            .field("items", &self.items.len())
            .finish_non_exhaustive()
    }
}

impl MediaCarousel {
    /// A carousel of `items`, sharing `cache` with the other images on screen.
    #[must_use]
    pub fn new(
        id: &str,
        state: Entity<CarouselState>,
        items: Vec<MediaCardView>,
        cache: Entity<LruImageCache>,
    ) -> Self {
        Self {
            prefix: id.to_owned(),
            state,
            items,
            cache,
        }
    }
}

impl RenderOnce for MediaCarousel {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let previous = {
            let state = self.state.clone();
            Button::new(ElementId::Name(format!("{}-previous", self.prefix).into()))
                .label("‹")
                .disabled(!self.state.read(cx).has_previous())
                .on_click(move |_, _, cx| {
                    let _ = page(&state, PageDirection::Previous, cx);
                })
        };
        let next = {
            let state = self.state.clone();
            Button::new(ElementId::Name(format!("{}-next", self.prefix).into()))
                .label("›")
                .disabled(!self.state.read(cx).has_next())
                .on_click(move |_, _, cx| {
                    let _ = page(&state, PageDirection::Next, cx);
                })
        };
        let cards = image_cache(self.cache.clone()).child(
            CarouselContent::new(&self.state)
                .h(px(MEDIA_CARD_HEIGHT))
                .children(self.items.into_iter().enumerate().map(|(index, view)| {
                    let item_id = ElementId::Name(format!("{}-item-{index}", self.prefix).into());
                    let card_id = ElementId::Name(format!("{}-card-{index}", self.prefix).into());
                    CarouselItem::new(item_id, index, &self.state)
                        .w(px(POSTER_WIDTH))
                        .child(MediaCard::new(view, card_id).width(px(POSTER_WIDTH)))
                })),
        );
        let wheel_state = self.state.clone();
        div()
            .id(ElementId::Name(self.prefix.clone().into()))
            .test_support()
            .flex()
            .flex_col()
            .gap_2()
            .on_scroll(move |event: &ScrollEvent, window, cx| {
                let Some(delta) = alt_wheel_delta(event, window.line_height()) else {
                    return;
                };
                let direction = if delta.x < px(0.) {
                    PageDirection::Next
                } else {
                    PageDirection::Previous
                };
                if page(&wheel_state, direction, cx) {
                    cx.stop_propagation();
                }
            })
            .child(
                Carousel::new(
                    ElementId::Name(format!("{}-track", self.prefix).into()),
                    &self.state,
                )
                .h(px(MEDIA_CARD_HEIGHT))
                .child(cards),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .gap_2()
                    .child(previous)
                    .child(next),
            )
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{AppContext as _, Modifiers, ScrollDelta, TestAppContext};

    use super::*;

    #[gpui_kit::test]
    fn page_one_item_and_stop_at_the_ends(cx: &mut TestAppContext) {
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
