//! The episode card and episode row: a 16:9 thumbnail, badges, click and
//! context-menu callbacks, and a continuity progress bar. Both take a plain
//! view model and render only what it says. See ticket #129.

use std::rc::Rc;

use gpui_kit::{
    App, Div, ElementId, Hsla, ImageSource, InteractiveElement, IntoElement, ObjectFit,
    ParentElement, Pixels, RenderOnce, Resource, SharedString, StatefulInteractiveElement, Styled,
    StyledImage, TestSupportExt, Window,
    component::{
        ActiveTheme, Sizable as _,
        menu::{ContextMenuExt, PopupMenu, PopupMenuItem},
        skeleton::Skeleton,
        tag::Tag,
    },
    div, img, px,
};

/// The card's default width; the thumbnail keeps a 16:9 ratio.
pub const EPISODE_CARD_WIDTH: f32 = 280.;
/// The row's default width; its thumbnail is smaller to leave room for text.
pub const EPISODE_ROW_WIDTH: f32 = 420.;
/// The row's thumbnail width.
const ROW_THUMB_WIDTH: f32 = 120.;
/// The progress bar's height, in pixels.
const PROGRESS_HEIGHT: f32 = 3.;

const THUMBNAIL: &str = "episode-card-thumbnail";
const PLACEHOLDER: &str = "episode-card-placeholder";
const SKELETON: &str = "episode-card-skeleton";
const NUMBER: &str = "episode-card-number";
const TITLE: &str = "episode-card-title";
const MEDIA_TITLE: &str = "episode-card-media-title";
const FILENAME: &str = "episode-card-filename";
const FILLER: &str = "episode-card-filler";
const WATCHED: &str = "episode-card-watched";
const PROGRESS_TRACK: &str = "episode-card-progress-track";
const PROGRESS_FILL: &str = "episode-card-progress-fill";

/// One episode as a card or row shows it: plain view-model data, no I/O.
#[expect(
    clippy::struct_excessive_bools,
    reason = "filler, watched, locked and skeleton are independent display flags from the view model"
)]
#[derive(Debug, Clone, Default)]
pub struct EpisodeCardView {
    /// The thumbnail to load through the surrounding bounded image cache.
    /// `None` draws the styled placeholder instead.
    pub thumbnail: Option<Resource>,
    /// The episode number in its pill.
    pub episode_number: u32,
    /// The episode title.
    pub title: SharedString,
    /// The media title, when the screen shows it.
    pub media_title: Option<SharedString>,
    /// The local file name, when the episode is in the library.
    pub filename: Option<SharedString>,
    /// Whether the episode is filler.
    pub filler: bool,
    /// Whether the episode has been watched.
    pub watched: bool,
    /// Whether the episode's files are locked.
    pub locked: bool,
    /// Continuity fraction in `0..=1`; `None` draws no progress bar.
    pub progress: Option<f32>,
    /// Whether the card is still loading and should draw its skeleton.
    pub skeleton: bool,
}

/// An action the card offers through a click or its context menu.
type EpisodeAction = Rc<dyn Fn(&mut Window, &mut App)>;

/// The four callbacks a card or row can offer. It holds no behavior of its
/// own: a screen decides what play, watched, lock and open folder mean.
#[derive(Clone, Default)]
pub struct EpisodeActions {
    play: Option<EpisodeAction>,
    mark_watched: Option<EpisodeAction>,
    toggle_lock: Option<EpisodeAction>,
    open_folder: Option<EpisodeAction>,
}

impl std::fmt::Debug for EpisodeActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpisodeActions")
            .field("play", &self.play.is_some())
            .field("mark_watched", &self.mark_watched.is_some())
            .field("toggle_lock", &self.toggle_lock.is_some())
            .field("open_folder", &self.open_folder.is_some())
            .finish()
    }
}

impl EpisodeActions {
    /// No actions. The card is presentational until a screen supplies them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The action run when the card is clicked and by Play in its menu.
    #[must_use]
    pub fn on_play(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.play = Some(Rc::new(action));
        self
    }

    /// The action run by Mark watched / Mark unwatched in the menu.
    #[must_use]
    pub fn on_mark_watched(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.mark_watched = Some(Rc::new(action));
        self
    }

    /// The action run by Lock / Unlock in the menu.
    #[must_use]
    pub fn on_toggle_lock(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.toggle_lock = Some(Rc::new(action));
        self
    }

    /// The action run by Open folder in the menu.
    #[must_use]
    pub fn on_open_folder(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.open_folder = Some(Rc::new(action));
        self
    }

    /// Fires Play, as a click on the card does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_play(&self, window: &mut Window, cx: &mut App) {
        if let Some(action) = &self.play {
            action(window, cx);
        }
    }

    /// Fires Mark watched, as the context menu does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_mark_watched(&self, window: &mut Window, cx: &mut App) {
        if let Some(action) = &self.mark_watched {
            action(window, cx);
        }
    }

    /// Fires Lock / Unlock, as the context menu does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_toggle_lock(&self, window: &mut Window, cx: &mut App) {
        if let Some(action) = &self.toggle_lock {
            action(window, cx);
        }
    }

    /// Fires Open folder, as the context menu does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_open_folder(&self, window: &mut Window, cx: &mut App) {
        if let Some(action) = &self.open_folder {
            action(window, cx);
        }
    }
}

/// The 16:9 episode card for grids and carousels.
#[derive(IntoElement)]
pub struct EpisodeCard {
    view: EpisodeCardView,
    id: ElementId,
    width: Pixels,
    actions: EpisodeActions,
}

impl std::fmt::Debug for EpisodeCard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpisodeCard")
            .field("view", &self.view)
            .field("id", &self.id)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl EpisodeCard {
    /// A card for `view` with `id`. Its width defaults to
    /// [`EPISODE_CARD_WIDTH`].
    pub fn new(view: EpisodeCardView, id: impl Into<ElementId>) -> Self {
        Self {
            view,
            id: id.into(),
            width: px(EPISODE_CARD_WIDTH),
            actions: EpisodeActions::new(),
        }
    }

    /// Sets the card's width; the thumbnail height follows from the ratio.
    #[must_use]
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Supplies the card's click and context-menu callbacks.
    #[must_use]
    pub fn actions(mut self, actions: EpisodeActions) -> Self {
        self.actions = actions;
        self
    }

    /// Fires Play, as a click on the card does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_play(&self, window: &mut Window, cx: &mut App) {
        self.actions.fire_play(window, cx);
    }

    /// Fires Mark watched, as the context menu does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_mark_watched(&self, window: &mut Window, cx: &mut App) {
        self.actions.fire_mark_watched(window, cx);
    }

    /// Fires Lock / Unlock, as the context menu does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_toggle_lock(&self, window: &mut Window, cx: &mut App) {
        self.actions.fire_toggle_lock(window, cx);
    }

    /// Fires Open folder, as the context menu does.
    #[cfg(any(test, feature = "test-util"))]
    pub fn fire_open_folder(&self, window: &mut Window, cx: &mut App) {
        self.actions.fire_open_folder(window, cx);
    }
}

impl RenderOnce for EpisodeCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.view.skeleton {
            return skeleton(self.width, thumbnail_height(self.width)).into_any_element();
        }

        let height = thumbnail_height(self.width);
        let thumbnail = match &self.view.thumbnail {
            Some(resource) => div()
                .absolute()
                .inset_0()
                .debug_selector(|| THUMBNAIL.into())
                .child(
                    img(ImageSource::Resource(resource.clone()))
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| placeholder_thumbnail().into_any_element())
                        .with_fallback(|| placeholder_thumbnail().into_any_element()),
                ),
            None => div()
                .absolute()
                .inset_0()
                .debug_selector(|| PLACEHOLDER.into())
                .child(placeholder_thumbnail()),
        };
        let card = div()
            .id(self.id.clone())
            .test_support()
            .w(self.width)
            .flex_none()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(height)
                    .overflow_hidden()
                    .rounded(px(8.))
                    .child(thumbnail)
                    .child(number_pill(&self.view))
                    .children(overlay_badges(&self.view))
                    .child(progress_track(
                        self.view.progress,
                        self.width,
                        cx.theme().colors.muted_foreground,
                    )),
            )
            .child(
                div()
                    .debug_selector(|| TITLE.into())
                    .text_sm()
                    .text_color(cx.theme().colors.foreground)
                    .child(self.view.title.clone()),
            )
            .children(self.view.media_title.as_ref().map(|media_title| {
                div()
                    .debug_selector(|| MEDIA_TITLE.into())
                    .text_xs()
                    .text_color(cx.theme().colors.muted_foreground)
                    .child(media_title.clone())
            }));

        card.on_click({
            let actions = self.actions.clone();
            move |_, window, cx| {
                if let Some(on_play) = &actions.play {
                    on_play(window, cx);
                }
            }
        })
        .context_menu(move |menu, _, _| context_menu_items(&self.view, &self.actions, menu))
        .into_any_element()
    }
}

/// A compact episode row for long lists, such as the Entry Library tab.
#[derive(IntoElement)]
pub struct EpisodeRow {
    view: EpisodeCardView,
    id: ElementId,
    width: Pixels,
    actions: EpisodeActions,
}

impl std::fmt::Debug for EpisodeRow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpisodeRow")
            .field("view", &self.view)
            .field("id", &self.id)
            .field("width", &self.width)
            .finish_non_exhaustive()
    }
}

impl EpisodeRow {
    /// A row for `view` with `id`. Its width defaults to [`EPISODE_ROW_WIDTH`].
    pub fn new(view: EpisodeCardView, id: impl Into<ElementId>) -> Self {
        Self {
            view,
            id: id.into(),
            width: px(EPISODE_ROW_WIDTH),
            actions: EpisodeActions::new(),
        }
    }

    /// Sets the row's width.
    #[must_use]
    pub fn width(mut self, width: Pixels) -> Self {
        self.width = width;
        self
    }

    /// Supplies the row's click and context-menu callbacks.
    #[must_use]
    pub fn actions(mut self, actions: EpisodeActions) -> Self {
        self.actions = actions;
        self
    }
}

impl RenderOnce for EpisodeRow {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.view.skeleton {
            return skeleton(self.width, thumbnail_height(px(ROW_THUMB_WIDTH))).into_any_element();
        }

        let colors = cx.theme().colors;
        let thumb_width = px(ROW_THUMB_WIDTH);
        let thumb_height = thumbnail_height(thumb_width);
        let thumbnail = match &self.view.thumbnail {
            Some(resource) => div()
                .absolute()
                .inset_0()
                .debug_selector(|| THUMBNAIL.into())
                .child(
                    img(ImageSource::Resource(resource.clone()))
                        .w_full()
                        .h_full()
                        .object_fit(ObjectFit::Cover)
                        .with_loading(|| placeholder_thumbnail().into_any_element())
                        .with_fallback(|| placeholder_thumbnail().into_any_element()),
                ),
            None => div()
                .absolute()
                .inset_0()
                .debug_selector(|| PLACEHOLDER.into())
                .child(placeholder_thumbnail()),
        };
        let row = div()
            .id(self.id.clone())
            .test_support()
            .w(self.width)
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .pb(px(PROGRESS_HEIGHT))
            .child(
                div()
                    .relative()
                    .w(thumb_width)
                    .h(thumb_height)
                    .flex_none()
                    .overflow_hidden()
                    .rounded(px(6.))
                    .child(thumbnail)
                    .child(number_pill(&self.view)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .debug_selector(|| TITLE.into())
                            .text_sm()
                            .text_color(colors.foreground)
                            .child(self.view.title.clone()),
                    )
                    .children(self.view.media_title.as_ref().map(|media_title| {
                        div()
                            .debug_selector(|| MEDIA_TITLE.into())
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(media_title.clone())
                    }))
                    .children(inline_badges(&self.view)),
            )
            .child(progress_track(
                self.view.progress,
                self.width,
                colors.muted_foreground,
            ));

        row.on_click({
            let actions = self.actions.clone();
            move |_, window, cx| {
                if let Some(on_play) = &actions.play {
                    on_play(window, cx);
                }
            }
        })
        .context_menu(move |menu, _, _| context_menu_items(&self.view, &self.actions, menu))
        .into_any_element()
    }
}

/// The thumbnail height for a 16:9 image of `width`.
fn thumbnail_height(width: Pixels) -> Pixels {
    px(f32::from(width) * 9. / 16.)
}

/// The styled stand-in for a thumbnail that is missing or still loading.
fn placeholder_thumbnail() -> Div {
    div()
        .flex()
        .items_center()
        .justify_center()
        .h_full()
        .w_full()
        .bg(gpui_kit::rgb(0x001b_1b22))
        .child(
            div()
                .text_xs()
                .text_color(gpui_kit::rgb(0x008a_8a99))
                .child("No thumbnail"),
        )
}

/// Shimmer rectangles in place of the whole card while its data loads.
fn skeleton(width: Pixels, height: Pixels) -> Div {
    div()
        .debug_selector(|| SKELETON.into())
        .w(width)
        .flex_none()
        .flex_col()
        .gap_2()
        .child(Skeleton::new().w_full().h(height))
        .child(Skeleton::new().w(px(f32::from(width) * 0.7)).h(px(12.)))
        .child(Skeleton::new().w(px(f32::from(width) * 0.45)).h(px(10.)))
}

/// The episode number pill over the thumbnail's top-left corner.
fn number_pill(view: &EpisodeCardView) -> Div {
    div()
        .absolute()
        .top(px(8.))
        .left(px(8.))
        .debug_selector(|| NUMBER.into())
        .rounded_full()
        .bg(gpui_kit::Hsla::black().opacity(0.7))
        .px(px(8.))
        .py(px(3.))
        .text_xs()
        .text_color(gpui_kit::rgb(0x00ff_ffff))
        .child(format!("EP {}", view.episode_number))
}

/// Badges laid over the thumbnail's bottom-left corner.
fn overlay_badges(view: &EpisodeCardView) -> Vec<Div> {
    let mut badges = Vec::new();
    if view.filler {
        badges.push(
            div()
                .absolute()
                .bottom(px(10.))
                .left(px(8.))
                .debug_selector(|| FILLER.into())
                .child(Tag::secondary().small().child("Filler")),
        );
    }
    if let Some(filename) = &view.filename {
        badges.push(
            div()
                .absolute()
                .bottom(px(10.))
                .right(px(8.))
                .max_w(px(140.))
                .debug_selector(|| FILENAME.into())
                .child(Tag::secondary().small().child(filename.clone())),
        );
    }
    if view.watched {
        badges.push(
            div()
                .absolute()
                .top(px(8.))
                .right(px(8.))
                .debug_selector(|| WATCHED.into())
                .child(Tag::primary().small().child("✓")),
        );
    }
    badges
}

/// The badges a row shows beside its title.
fn inline_badges(view: &EpisodeCardView) -> Vec<Div> {
    let mut badges = Vec::new();
    if view.filler {
        badges.push(
            div()
                .debug_selector(|| FILLER.into())
                .flex_none()
                .child(Tag::secondary().small().child("Filler")),
        );
    }
    if view.watched {
        badges.push(
            div()
                .debug_selector(|| WATCHED.into())
                .flex_none()
                .child(Tag::primary().small().child("✓ Watched")),
        );
    }
    if let Some(filename) = &view.filename {
        badges.push(
            div()
                .debug_selector(|| FILENAME.into())
                .flex_none()
                .child(Tag::secondary().small().child(filename.clone())),
        );
    }
    badges
}

/// Continuity bar along the bottom edge of the thumbnail or row.
fn progress_track(progress: Option<f32>, width: Pixels, track_color: Hsla) -> Div {
    div()
        .debug_selector(|| PROGRESS_TRACK.into())
        .absolute()
        .bottom(px(0.))
        .left(px(0.))
        .right(px(0.))
        .h(px(PROGRESS_HEIGHT))
        .overflow_hidden()
        .bg(track_color.opacity(0.25))
        .children(progress.map(|fraction| {
            div()
                .debug_selector(|| PROGRESS_FILL.into())
                .h_full()
                .w(px(f32::from(width) * fraction.clamp(0., 1.)))
                .bg(gpui_kit::rgb(0x0061_52df))
        }))
}

/// The context menu every card and row offers, with only the actions a
/// screen supplied.
fn context_menu_items(
    view: &EpisodeCardView,
    actions: &EpisodeActions,
    mut menu: PopupMenu,
) -> PopupMenu {
    if let Some(on_play) = actions.play.clone() {
        menu = menu.item(PopupMenuItem::new("Play").on_click(move |_, window, cx| {
            on_play(window, cx);
        }));
    }
    if let Some(on_mark_watched) = actions.mark_watched.clone() {
        let label = if view.watched {
            "Mark unwatched"
        } else {
            "Mark watched"
        };
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
            on_mark_watched(window, cx);
        }));
    }
    if let Some(on_toggle_lock) = actions.toggle_lock.clone() {
        let label = if view.locked { "Unlock" } else { "Lock" };
        menu = menu.item(PopupMenuItem::new(label).on_click(move |_, window, cx| {
            on_toggle_lock(window, cx);
        }));
    }
    if let Some(on_open_folder) = actions.open_folder.clone() {
        menu = menu.item(
            PopupMenuItem::new("Open folder").on_click(move |_, window, cx| {
                on_open_folder(window, cx);
            }),
        );
    }
    menu
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui_kit::{Context, IntoElement, Render, TestAppContext};

    use super::*;

    fn view() -> EpisodeCardView {
        EpisodeCardView {
            thumbnail: Some(Resource::Uri("https://example.com/episode.jpg".into())),
            episode_number: 7,
            title: "The Stranger at the Window".into(),
            media_title: Some("Frieren: Beyond Journey's End".into()),
            filename: Some("[Group] Frieren - 07.mkv".into()),
            filler: false,
            watched: false,
            locked: false,
            progress: Some(0.4),
            skeleton: false,
        }
    }

    struct CardView {
        view: EpisodeCardView,
        id: &'static str,
    }

    impl Render for CardView {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            EpisodeCard::new(self.view.clone(), self.id)
        }
    }

    #[gpui_kit::test]
    fn card_shows_the_thumbnail_number_titles_and_progress(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: view(),
            id: "episode-card-test",
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        for selector in [
            THUMBNAIL,
            NUMBER,
            TITLE,
            MEDIA_TITLE,
            FILENAME,
            PROGRESS_TRACK,
            PROGRESS_FILL,
        ] {
            assert!(
                cx.debug_bounds(selector).is_some(),
                "the card shows {selector}"
            );
        }
        assert!(
            cx.debug_bounds(FILLER).is_none(),
            "a non-filler episode has no filler badge"
        );
        assert!(
            cx.debug_bounds(WATCHED).is_none(),
            "an unwatched episode has no checkmark"
        );
    }

    #[gpui_kit::test]
    fn missing_thumbnail_draws_a_placeholder_and_skeleton_draws_shimmer(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: EpisodeCardView {
                thumbnail: None,
                ..view()
            },
            id: "episode-card-placeholder-test",
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds(PLACEHOLDER).is_some(),
            "a missing thumbnail draws the placeholder"
        );
        assert!(cx.debug_bounds(THUMBNAIL).is_none());

        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: EpisodeCardView {
                skeleton: true,
                ..view()
            },
            id: "episode-card-skeleton-test",
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.debug_bounds(SKELETON).is_some(),
            "the skeleton state draws shimmer rectangles"
        );
        assert!(cx.debug_bounds(TITLE).is_none());
    }

    #[gpui_kit::test]
    fn progress_bar_width_follows_the_view_model(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);

        let measured = |view: EpisodeCardView, id: &'static str, cx: &mut TestAppContext| {
            let (_root, cx) = cx.add_window_view(move |_, _| CardView { view, id });
            cx.update(|window, cx| window.draw(cx).clear(cx));
            let fill = cx.debug_bounds(PROGRESS_FILL).expect("progress fill");
            let track = cx.debug_bounds(PROGRESS_TRACK).expect("progress track");
            f32::from(fill.size.width) / f32::from(track.size.width)
        };

        let partial = measured(view(), "episode-card-progress-partial", cx);
        let complete = measured(
            EpisodeCardView {
                progress: Some(0.8),
                ..view()
            },
            "episode-card-progress-complete",
            cx,
        );
        assert!(
            (partial - 0.4).abs() < 0.02,
            "the fill is 40% of the track, got {partial}"
        );
        assert!(
            (complete - 0.8).abs() < 0.02,
            "the fill is 80% of the track, got {complete}"
        );
    }

    #[gpui_kit::test]
    fn no_progress_means_no_bar(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: EpisodeCardView {
                progress: None,
                ..view()
            },
            id: "episode-card-no-progress-test",
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds(PROGRESS_FILL).is_none());
    }

    #[gpui_kit::test]
    fn all_four_callbacks_fire(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let calls = Rc::new(Cell::new(0u32));
        let actions = EpisodeActions::new()
            .on_play({
                let calls = Rc::clone(&calls);
                move |_, _| calls.set(calls.get() + 1)
            })
            .on_mark_watched({
                let calls = Rc::clone(&calls);
                move |_, _| calls.set(calls.get() + 10)
            })
            .on_toggle_lock({
                let calls = Rc::clone(&calls);
                move |_, _| calls.set(calls.get() + 100)
            })
            .on_open_folder({
                let calls = Rc::clone(&calls);
                move |_, _| calls.set(calls.get() + 1000)
            });
        let card = EpisodeCard::new(view(), "episode-card-callbacks").actions(actions);
        let (_root, cx) = cx.add_window_view(|_, _| CardView {
            view: view(),
            id: "episode-card-callback-window",
        });
        cx.update(|window, cx| {
            card.fire_play(window, cx);
            card.fire_mark_watched(window, cx);
            card.fire_toggle_lock(window, cx);
            card.fire_open_folder(window, cx);
        });
        assert_eq!(calls.get(), 1111, "every callback fired once");
    }
}
