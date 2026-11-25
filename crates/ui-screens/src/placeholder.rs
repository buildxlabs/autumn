//! Stand-in screens for routes whose real screen comes in a later phase.

use gpui_kit::{
    ElementId, Entity, Hsla, IntoElement, ParentElement, Resource, SharedString, Styled, div,
    image_cache, px,
};
use ui_components::{
    EpisodeCard, EpisodeCardView, EpisodeRow, LruImageCache, MediaCardView, MediaFormat,
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

/// Mock episodes covering every state the card renders: unwatched, in
/// progress, watched, filler, no thumbnail and skeleton.
pub(crate) fn mock_episode_views() -> Vec<EpisodeCardView> {
    let base = EpisodeCardView {
        thumbnail: Some(Resource::Uri(
            format!("{SAMPLE_POSTER}?episode=mock").into(),
        )),
        episode_number: 7,
        title: "The Stranger at the Window".into(),
        media_title: Some("Frieren: Beyond Journey's End".into()),
        filename: Some("[Group] Frieren - 07.mkv".into()),
        ..EpisodeCardView::default()
    };
    vec![
        EpisodeCardView {
            episode_number: 1,
            title: "Unwatched".into(),
            ..base.clone()
        },
        EpisodeCardView {
            episode_number: 2,
            title: "In progress (40%)".into(),
            progress: Some(0.4),
            ..base.clone()
        },
        EpisodeCardView {
            episode_number: 3,
            title: "Watched".into(),
            watched: true,
            progress: Some(1.0),
            ..base.clone()
        },
        EpisodeCardView {
            episode_number: 4,
            title: "Filler".into(),
            filler: true,
            ..base.clone()
        },
        EpisodeCardView {
            episode_number: 5,
            title: "No thumbnail".into(),
            thumbnail: None,
            ..base.clone()
        },
        EpisodeCardView {
            episode_number: 6,
            title: "Loading".into(),
            skeleton: true,
            ..base
        },
    ]
}

/// The component gallery on the Schedule placeholder route, until its real
/// screen lands. Both layouts use the bounded image cache.
pub(crate) fn episode_gallery(cache: &Entity<LruImageCache>) -> impl IntoElement {
    let views = mock_episode_views();
    div()
        .p_8()
        .flex()
        .flex_col()
        .gap_8()
        .child(div().text_xl().child("Episode cards"))
        .child(
            image_cache(cache.clone())
                .flex()
                .flex_wrap()
                .gap_4()
                .children(views.iter().enumerate().map(|(ix, view)| {
                    EpisodeCard::new(
                        view.clone(),
                        ElementId::Name(format!("mock-episode-card-{ix}").into()),
                    )
                })),
        )
        .child(div().text_xl().child("Episode rows"))
        .child(
            image_cache(cache.clone())
                .flex()
                .flex_col()
                .gap_2()
                .children(views.iter().enumerate().map(|(ix, view)| {
                    EpisodeRow::new(
                        view.clone(),
                        ElementId::Name(format!("mock-episode-row-{ix}").into()),
                    )
                })),
        )
}
