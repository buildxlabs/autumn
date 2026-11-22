//! Stand-in screens for routes whose real screen comes in a later phase.

use gpui_kit::{
    ElementId, Hsla, InteractiveElement, IntoElement, ParentElement, SharedString, Styled,
    TestSupportExt, div, px,
};

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

/// How many cards [`poster_grid`] draws.
const POSTERS: usize = 24;

/// A wrapping grid of poster-sized cards, for checking that the rail does not
/// reflow content. Each card's id is `poster-<n>`, counting from 1.
pub(crate) fn poster_grid(fill: Hsla, border: Hsla) -> impl IntoElement {
    div()
        .px_8()
        .pb_8()
        .flex()
        .flex_wrap()
        .gap_4()
        .children((1..=POSTERS).map(move |n| {
            div()
                .id(ElementId::Name(format!("poster-{n}").into()))
                .test_support()
                .w(px(160.))
                .h(px(240.))
                .rounded(px(8.))
                .bg(fill)
                .border_1()
                .border_color(border)
        }))
}
