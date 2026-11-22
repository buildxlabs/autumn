//! Stand-in screens for routes whose real screen comes in a later phase.

use gpui_kit::{Hsla, IntoElement, ParentElement, SharedString, Styled, div};

/// A titled, empty screen.
pub(crate) fn placeholder(title: SharedString, muted: Hsla) -> impl IntoElement {
    div()
        .p_8()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_3xl().child(title))
        .child(div().text_sm().text_color(muted).child("Nothing here yet."))
}
