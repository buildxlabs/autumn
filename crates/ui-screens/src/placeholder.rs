//! Stand-in screens for routes whose real screen comes in a later phase.

use gpui_kit::{Hsla, IntoElement, ParentElement, SharedString, Styled, div, px};

/// A titled screen; with `rows`, a long list to scroll through.
pub(crate) fn placeholder(title: SharedString, rows: usize, muted: Hsla) -> impl IntoElement {
    div()
        .p_8()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_3xl().child(title.clone()))
        .child(div().text_sm().text_color(muted).child("Nothing here yet."))
        .children((1..=rows).map(move |row| {
            div()
                .h(px(48.))
                .flex()
                .items_center()
                .child(format!("{title} {row}"))
        }))
}
