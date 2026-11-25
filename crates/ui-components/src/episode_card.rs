//! The episode card and episode row: a 16:9 thumbnail, badges, click and
//! context-menu callbacks, and a continuity progress bar. Both take a plain
//! view model and render only what it says. See ticket #129.

use std::rc::Rc;

use gpui_kit::{
    App, ClickEvent, Div, ElementId, Hsla, ImageSource, InteractiveElement, IntoElement, Modifiers,
    ObjectFit, ParentElement, Pixels, RenderOnce, Resource, SharedString,
    StatefulInteractiveElement, Styled, StyledImage, TestSupportExt, Window,
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
const UNAVAILABLE: &str = "episode-card-unavailable";
const SELECTED: &str = "episode-card-selected";

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
    /// Why the episode can't play, such as a disconnected drive. The card
    /// shows the reason and ignores clicks and Play.
    pub unavailable: Option<SharedString>,
    /// Whether the card is part of the screen's selection.
    pub selected: bool,
}

/// An action the card offers through a click or its context menu.
type EpisodeAction = Rc<dyn Fn(&mut Window, &mut App)>;
/// A click handler that sees the click's modifiers, for selection.
type ClickAction = Rc<dyn Fn(Modifiers, &mut Window, &mut App)>;

/// The four callbacks a card or row can offer. It holds no behavior of its
/// own: a screen decides what play, watched, lock and open folder mean.
#[derive(Clone, Default)]
pub struct EpisodeActions {
    play: Option<EpisodeAction>,
    mark_watched: Option<EpisodeAction>,
    toggle_lock: Option<EpisodeAction>,
    open_folder: Option<EpisodeAction>,
    click: Option<ClickAction>,
}

impl std::fmt::Debug for EpisodeActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EpisodeActions")
            .field("play", &self.play.is_some())
            .field("mark_watched", &self.mark_watched.is_some())
            .field("toggle_lock", &self.toggle_lock.is_some())
            .field("open_folder", &self.open_folder.is_some())
            .field("click", &self.click.is_some())
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

    /// The action run by a click instead of Play, given the click's
    /// modifiers. Play stays in the menu.
    #[must_use]
    pub fn on_click(mut self, action: impl Fn(Modifiers, &mut Window, &mut App) + 'static) -> Self {
        self.click = Some(Rc::new(action));
        self
    }

    /// The action run by Open folder in the menu.
    #[must_use]
    pub fn on_open_folder(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.open_folder = Some(Rc::new(action));
        self
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
}

impl RenderOnce for EpisodeCard {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        if self.view.skeleton {
            return skeleton(self.id, self.width, thumbnail_height(self.width)).into_any_element();
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
                    .children(self.view.unavailable.clone().map(unavailable_overlay))
                    .children(self.view.selected.then(|| selection_ring(cx)))
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
            let available = self.view.unavailable.is_none();
            move |event: &ClickEvent, window, cx| {
                if let Some(on_click) = &actions.click {
                    on_click(event.modifiers(), window, cx);
                } else if let (true, Some(on_play)) = (available, &actions.play) {
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
            return skeleton(self.id, self.width, thumbnail_height(px(ROW_THUMB_WIDTH)))
                .into_any_element();
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
                    .child(number_pill(&self.view))
                    .children(self.view.selected.then(|| selection_ring(cx))),
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
            let available = self.view.unavailable.is_none();
            move |event: &ClickEvent, window, cx| {
                if let Some(on_click) = &actions.click {
                    on_click(event.modifiers(), window, cx);
                } else if let (true, Some(on_play)) = (available, &actions.play) {
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
fn skeleton(id: ElementId, width: Pixels, height: Pixels) -> impl IntoElement {
    div()
        .id(id)
        .test_support()
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
    if let Some(reason) = &view.unavailable {
        badges.push(
            div()
                .debug_selector(|| UNAVAILABLE.into())
                .flex_none()
                .child(Tag::danger().small().child(reason.clone())),
        );
    }
    badges
}

/// The accent ring over a selected card's thumbnail.
fn selection_ring(cx: &App) -> Div {
    div()
        .absolute()
        .inset_0()
        .rounded(px(6.))
        .border_2()
        .border_color(cx.theme().colors.accent)
        .debug_selector(|| SELECTED.into())
}

/// The dimmed cover over an unavailable card's thumbnail, with the reason.
fn unavailable_overlay(reason: SharedString) -> Div {
    div()
        .absolute()
        .inset_0()
        .flex()
        .items_center()
        .justify_center()
        .bg(gpui_kit::Hsla::black().opacity(0.65))
        .debug_selector(|| UNAVAILABLE.into())
        .child(
            div()
                .text_xs()
                .text_color(gpui_kit::rgb(0x00ff_ffff))
                .child(reason),
        )
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
    if let (None, Some(on_play)) = (&view.unavailable, actions.play.clone()) {
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
    use std::cell::RefCell;

    use gpui_kit::{
        AppContext as _, Context, IntoElement, Render, TestAppContext, test::TestWindowExt,
    };

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
            unavailable: None,
            selected: false,
        }
    }

    struct CardView {
        view: EpisodeCardView,
        id: &'static str,
        actions: EpisodeActions,
    }

    impl Render for CardView {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            EpisodeCard::new(self.view.clone(), self.id).actions(self.actions.clone())
        }
    }

    struct RowView {
        view: EpisodeCardView,
        id: &'static str,
        actions: EpisodeActions,
    }

    impl Render for RowView {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            EpisodeRow::new(self.view.clone(), self.id).actions(self.actions.clone())
        }
    }

    #[gpui_kit::test]
    fn card_shows_the_thumbnail_number_titles_and_progress(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: view(),
            id: "episode-card-test",
            actions: EpisodeActions::new(),
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
    fn an_unavailable_episode_shows_why_and_does_not_play(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let played = Rc::new(RefCell::new(0));
        let actions = EpisodeActions::new().on_play({
            let played = Rc::clone(&played);
            move |_, _| *played.borrow_mut() += 1
        });
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: EpisodeCardView {
                unavailable: Some("Drive disconnected".into()),
                ..view()
            },
            id: "episode-card-unavailable-test",
            actions,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        let reason = cx.debug_bounds(UNAVAILABLE).expect("the reason is shown");
        cx.simulate_click(reason.center(), gpui_kit::Modifiers::none());
        assert_eq!(*played.borrow(), 0, "an unavailable card never plays");
    }

    #[gpui_kit::test]
    fn a_click_handler_sees_the_modifiers_and_a_selected_card_is_marked(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let clicks = Rc::new(RefCell::new(Vec::new()));
        let played = Rc::new(RefCell::new(0));
        let actions = EpisodeActions::new()
            .on_play({
                let played = Rc::clone(&played);
                move |_, _| *played.borrow_mut() += 1
            })
            .on_click({
                let clicks = Rc::clone(&clicks);
                move |modifiers, _, _| clicks.borrow_mut().push(modifiers)
            });
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: EpisodeCardView {
                selected: true,
                ..view()
            },
            id: "episode-card-select-test",
            actions,
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            cx.debug_bounds(SELECTED).is_some(),
            "the selection is shown"
        );
        let title = cx.debug_bounds(TITLE).expect("the title");
        cx.simulate_click(title.center(), gpui_kit::Modifiers::shift());
        assert_eq!(*clicks.borrow(), [gpui_kit::Modifiers::shift()]);
        assert_eq!(*played.borrow(), 0, "the click handler replaces play");
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
            actions: EpisodeActions::new(),
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
            actions: EpisodeActions::new(),
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
            let (_root, cx) = cx.add_window_view(move |_, _| CardView {
                view,
                id,
                actions: EpisodeActions::new(),
            });
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
    fn no_progress_means_no_fill(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let (_root, cx) = cx.add_window_view(move |_, _| CardView {
            view: EpisodeCardView {
                progress: None,
                ..view()
            },
            id: "episode-card-no-progress-test",
            actions: EpisodeActions::new(),
        });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(cx.debug_bounds(PROGRESS_FILL).is_none());
    }

    #[gpui_kit::test]
    fn click_and_context_menu_fire_the_callbacks(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let calls = Rc::new(RefCell::new(Vec::<&'static str>::new()));
        let actions = EpisodeActions::new()
            .on_play({
                let calls = Rc::clone(&calls);
                move |_, _| calls.borrow_mut().push("play")
            })
            .on_mark_watched({
                let calls = Rc::clone(&calls);
                move |_, _| calls.borrow_mut().push("mark watched")
            })
            .on_toggle_lock({
                let calls = Rc::clone(&calls);
                move |_, _| calls.borrow_mut().push("toggle lock")
            })
            .on_open_folder({
                let calls = Rc::clone(&calls);
                move |_, _| calls.borrow_mut().push("open folder")
            });

        // A Root is needed so the context menu's overlay can render.
        let window = cx.add_window(|window, cx| {
            let card = cx.new(|_| CardView {
                view: view(),
                id: "episode-card-callbacks",
                actions,
            });
            gpui_kit::base::Root::new(card, window, cx)
        });
        cx.update_window(window.into(), |_, window, cx| window.render_frame(cx))
            .expect("render the card");

        cx.update_window(window.into(), |_, window, cx| {
            window.click("episode-card-callbacks", cx);
        })
        .expect("click the card");
        assert_eq!(&*calls.borrow(), &["play"], "a click plays");

        for index in 0..4 {
            cx.update_window(window.into(), |_, window, cx| {
                window.right_click("episode-card-callbacks", cx);
            })
            .expect("right-click the card");
            cx.run_until_parked();
            cx.update_window(window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.within("popup-menu").click(index, cx);
            })
            .expect("pick a context-menu item");
        }
        assert_eq!(
            &*calls.borrow(),
            &["play", "play", "mark watched", "toggle lock", "open folder"],
            "each context-menu item fires its callback"
        );

        // The row wires the same actions through its own click and menu.
        calls.borrow_mut().clear();
        let row_actions = EpisodeActions::new()
            .on_play({
                let calls = Rc::clone(&calls);
                move |_, _| calls.borrow_mut().push("play")
            })
            .on_open_folder({
                let calls = Rc::clone(&calls);
                move |_, _| calls.borrow_mut().push("open folder")
            });
        let window = cx.add_window(|window, cx| {
            let row = cx.new(|_| RowView {
                view: view(),
                id: "episode-row-callbacks",
                actions: row_actions,
            });
            gpui_kit::base::Root::new(row, window, cx)
        });
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click("episode-row-callbacks", cx);
        })
        .expect("click the row");
        cx.update_window(window.into(), |_, window, cx| {
            window.right_click("episode-row-callbacks", cx);
        })
        .expect("right-click the row");
        cx.run_until_parked();
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.within("popup-menu").click(1, cx);
        })
        .expect("pick Open folder");
        assert_eq!(
            &*calls.borrow(),
            &["play", "open folder"],
            "the row uses the same callbacks"
        );
    }
}
