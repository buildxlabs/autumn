//! Stand-in screens for routes whose real screen comes in a later phase.

use gpui_kit::{Hsla, IntoElement, ParentElement, Resource, SharedString, Styled, div, px};
use ui_components::{MediaCardView, MediaFormat};

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

/// Frieren's AniList cover, until Home renders real catalog data.
const SAMPLE_POSTER: &str =
    "https://s4.anilist.co/file/anilistcdn/media/anime/cover/large/bx154587-qQTzQnEJJ3oB.jpg";

/// How many mock cards the placeholder grid shows: enough to exercise the
/// grid's virtualization.
const MOCK_COUNT: usize = 2000;

/// Mock media for the Home grid, until Home renders real catalog data.
pub(crate) fn mock_media_views() -> Vec<MediaCardView> {
    (0..MOCK_COUNT)
        .map(|ix| MediaCardView {
            poster: Some(Resource::Uri(format!("{SAMPLE_POSTER}?mock={ix}").into())),
            title: format!("Mock Series {ix}").into(),
            format: Some(MediaFormat::Anime),
            score: Some(8.0),
            progress: 0,
            total: Some(12),
            detail: "A mock media card detail.".into(),
        })
        .collect()
}
