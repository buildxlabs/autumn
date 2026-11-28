//! The buffer overlay shown in the playback bar while a torrent stream is
//! `Preparing` (#219, #182 Q8–Q9).
//!
//! It takes plain view models and callbacks, so it can be tested without an
//! engine (ARCHITECTURE §3). The same 60 px slot carries the normal HUD once
//! the player opens.

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    App, ClickEvent, Hsla, ImageSource, InteractiveElement, IntoElement, ObjectFit, ParentElement,
    RenderOnce, Resource, SharedString, Styled, StyledImage, TestSupportExt, Window,
    base::{Disableable as _, StyledExt as _},
    component::{ActiveTheme, Sizable as _, button::Button, label::Label},
    div, hsla, img, px, relative,
};

use crate::playback_hud::PLAYBACK_HUD_HEIGHT;

/// The poster, the stage trail and the readiness bars all live in this bar.
const POSTER_WIDTH: f32 = 36.;
const POSTER_HEIGHT: f32 = 48.;
const BAR_HEIGHT: f32 = 4.;
const BAR_WIDTH: f32 = 72.;

const NO_PEERS: &str = "buffer-overlay-no-peers";
const ELAPSED: &str = "buffer-overlay-elapsed";
const HEAD_BAR: &str = "buffer-overlay-head-bar";
const TAIL_BAR: &str = "buffer-overlay-tail-bar";
const HEAD_FILL: &str = "buffer-overlay-head-fill";
const TAIL_FILL: &str = "buffer-overlay-tail-fill";
const STAGE_METADATA: &str = "buffer-overlay-stage-metadata";
const STAGE_PEERS: &str = "buffer-overlay-stage-peers";
const STAGE_BUFFERING: &str = "buffer-overlay-stage-buffering";
const STATS: &str = "buffer-overlay-stats";
const CANCEL: &str = "buffer-overlay-cancel";
const CHOOSE_ANOTHER: &str = "buffer-overlay-choose-another";

/// The stage of the readiness gate, mirroring the engine's `PrepareStage`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum BufferStage {
    /// Waiting for metadata from peers.
    #[default]
    Metadata,
    /// Metadata is known, but no peer has connected.
    Peers,
    /// Downloading the head and tail windows.
    Buffering,
}

/// Downloaded pieces of one readiness window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PieceProgressView {
    /// Pieces downloaded and verified.
    pub done: u32,
    /// Pieces in the window; zero while metadata is unknown.
    pub total: u32,
}

/// Everything the buffer overlay draws, supplied by the store and the shell.
#[derive(Debug, Clone, Default)]
pub struct BufferOverlayView {
    /// The media's poster, when one is known.
    pub poster: Option<Resource>,
    /// The media's title.
    pub media_title: SharedString,
    /// The episode's number in the main run, when known.
    pub episode: Option<u32>,
    /// The episode's title, when known.
    pub episode_title: Option<SharedString>,
    /// Metadata, peers or buffering.
    pub stage: BufferStage,
    /// Progress of the 8 MiB head window.
    pub head: PieceProgressView,
    /// Progress of the 4 MiB tail window.
    pub tail: PieceProgressView,
    /// Connected peers.
    pub peers: u32,
    /// Download speed in bytes per second.
    pub download_bps: u64,
    /// Time spent preparing, shown once no peer has connected.
    pub elapsed: Duration,
    /// No peer has connected for 60 s (#179 Q2).
    pub no_peers: bool,
}

/// One button callback: the shell updates its stores from it.
type OverlayCallback = Rc<dyn Fn(&mut Window, &mut App)>;

/// Callbacks the shell wires to the playback store and the router.
#[derive(Clone, Default)]
pub struct BufferOverlayActions {
    cancel: Option<OverlayCallback>,
    choose_another: Option<OverlayCallback>,
}

impl std::fmt::Debug for BufferOverlayActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferOverlayActions")
            .field("cancel", &self.cancel.is_some())
            .field("choose_another", &self.choose_another.is_some())
            .finish()
    }
}

impl BufferOverlayActions {
    /// No callbacks: the overlay is presentational until a shell supplies them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Called when Cancel drops the stream.
    #[must_use]
    pub fn on_cancel(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.cancel = Some(Rc::new(action));
        self
    }

    /// Called when Choose another release drops the stream and opens the
    /// release list.
    #[must_use]
    pub fn on_choose_another(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.choose_another = Some(Rc::new(action));
        self
    }
}

/// The preparing bar element.
#[derive(IntoElement)]
pub struct BufferOverlay {
    id: SharedString,
    view: BufferOverlayView,
    actions: BufferOverlayActions,
}

impl BufferOverlay {
    /// Creates a bar with `id`, `view` and `actions`.
    pub fn new(
        id: impl Into<SharedString>,
        view: BufferOverlayView,
        actions: BufferOverlayActions,
    ) -> Self {
        Self {
            id: id.into(),
            view,
            actions,
        }
    }
}

impl std::fmt::Debug for BufferOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BufferOverlay")
            .field("id", &self.id)
            .field("view", &self.view)
            .field("actions", &self.actions)
            .finish()
    }
}

impl RenderOnce for BufferOverlay {
    #[expect(
        clippy::too_many_lines,
        reason = "the bar lays out the poster, titles, stage trail, windows and controls in one tree"
    )]
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let amber = amber();
        let title_line = episode_line(&self.view);
        let (head, tail) = (self.view.head, self.view.tail);

        div()
            .id(self.id)
            .test_support()
            .h(px(PLAYBACK_HUD_HEIGHT))
            .flex_none()
            .flex()
            .items_center()
            .gap_3()
            .px_4()
            .border_t_1()
            .border_color(colors.border)
            .bg(colors.sidebar)
            .child(poster(&self.view))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .justify_center()
                    .gap_1()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .min_w_0()
                            .child(
                                div()
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .child(Label::new(self.view.media_title.clone())),
                            )
                            .child(
                                div()
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(Label::new(title_line)),
                            ),
                    )
                    .child(stage_trail(
                        self.view.stage,
                        colors.foreground,
                        colors.muted_foreground,
                    ))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(window_bar(
                                HEAD_BAR,
                                HEAD_FILL,
                                "Head 8 MiB",
                                head,
                                colors.muted_foreground,
                                colors.border,
                                colors.primary,
                            ))
                            .child(window_bar(
                                TAIL_BAR,
                                TAIL_FILL,
                                "Tail 4 MiB",
                                tail,
                                colors.muted_foreground,
                                colors.border,
                                colors.primary,
                            )),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap_1()
                    .child(
                        div()
                            .id(STATS)
                            .debug_selector(|| STATS.into())
                            .text_xs()
                            .text_color(colors.muted_foreground)
                            .child(Label::new(format!(
                                "{} peers · {}",
                                self.view.peers,
                                speed(self.view.download_bps)
                            ))),
                    )
                    .children(self.view.no_peers.then(|| {
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .id(NO_PEERS)
                                    .test_support()
                                    .debug_selector(|| NO_PEERS.into())
                                    .text_xs()
                                    .text_color(amber)
                                    .child(Label::new("No peers found. Try another release.")),
                            )
                            .child(
                                div()
                                    .id(ELAPSED)
                                    .test_support()
                                    .debug_selector(|| ELAPSED.into())
                                    .text_xs()
                                    .text_color(amber)
                                    .child(Label::new(format!(
                                        "{} elapsed",
                                        clock(self.view.elapsed)
                                    ))),
                            )
                    })),
            )
            .children(self.view.no_peers.then(|| {
                text_button(
                    CHOOSE_ANOTHER,
                    "Choose another release",
                    self.actions.choose_another.as_ref(),
                )
            }))
            .child(text_button(CANCEL, "Cancel", self.actions.cancel.as_ref()))
    }
}

/// The warning colour for the no-peers hint and the in-HUD buffering chip.
pub(crate) fn amber() -> Hsla {
    hsla(38., 0.92, 0.62, 1.)
}

fn poster(view: &BufferOverlayView) -> impl IntoElement {
    match &view.poster {
        Some(poster) => div()
            .flex_none()
            .w(px(POSTER_WIDTH))
            .h(px(POSTER_HEIGHT))
            .rounded_sm()
            .overflow_hidden()
            .child(
                img(ImageSource::Resource(poster.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover),
            ),
        None => div()
            .flex_none()
            .w(px(POSTER_WIDTH))
            .h(px(POSTER_HEIGHT))
            .rounded_sm()
            .bg(Hsla::default()),
    }
}

fn stage_trail(stage: BufferStage, active: Hsla, inactive: Hsla) -> impl IntoElement {
    let stages = [
        (BufferStage::Metadata, "metadata", STAGE_METADATA),
        (BufferStage::Peers, "peers", STAGE_PEERS),
        (BufferStage::Buffering, "buffering", STAGE_BUFFERING),
    ];
    div()
        .flex()
        .items_center()
        .gap_1()
        .children(stages.into_iter().enumerate().flat_map(|(index, item)| {
            let (kind, label, selector) = item;
            let current = stage == kind;
            let marker = div()
                .debug_selector(move || selector.into())
                .text_xs()
                .text_color(if current { active } else { inactive })
                .child(Label::new(if current {
                    format!("▸ {label}")
                } else {
                    label.to_owned()
                }));
            if index == 0 {
                vec![marker.into_any_element()]
            } else {
                vec![
                    div()
                        .text_xs()
                        .text_color(inactive)
                        .child(Label::new("›"))
                        .into_any_element(),
                    marker.into_any_element(),
                ]
            }
        }))
}

fn window_bar(
    id: &'static str,
    fill_id: &'static str,
    label: &str,
    progress: PieceProgressView,
    muted: Hsla,
    border: Hsla,
    fill: Hsla,
) -> impl IntoElement {
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(
            div()
                .text_xs()
                .text_color(muted)
                .child(Label::new(if progress.total == 0 {
                    label.to_owned()
                } else {
                    format!("{label} {}/{}", progress.done, progress.total)
                })),
        )
        .child(
            div()
                .id(id)
                .test_support()
                .debug_selector(move || id.into())
                .relative()
                .h(px(BAR_HEIGHT))
                .w(px(BAR_WIDTH))
                .rounded_full()
                .bg(border)
                .child(
                    div()
                        .id(fill_id)
                        .test_support()
                        .debug_selector(move || fill_id.into())
                        .absolute()
                        .top_0()
                        .left_0()
                        .h_full()
                        .w(relative(fraction(progress)))
                        .rounded_full()
                        .bg(fill),
                ),
        )
}

fn text_button(
    id: &'static str,
    label: &'static str,
    action: Option<&OverlayCallback>,
) -> impl IntoElement {
    let button = Button::new(id)
        .small()
        .child(Label::new(label))
        .disabled(action.is_none());
    match action {
        Some(action) => {
            let action = Rc::clone(action);
            button
                .on_click(move |_: &ClickEvent, window, cx| action(window, cx))
                .into_any_element()
        }
        None => button.into_any_element(),
    }
}

/// The media title and episode line.
fn episode_line(view: &BufferOverlayView) -> String {
    let number = view.episode.map(|episode| format!("EP {episode}"));
    let title = view.episode_title.as_deref().unwrap_or_default();
    match (number, title.is_empty()) {
        (Some(number), false) => format!("{number} · {title}"),
        (Some(number), true) => number,
        (None, false) => title.to_owned(),
        (None, true) => String::new(),
    }
}

/// Downloaded over total pieces, clamped to `0..=1`.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "piece counts are display values; a bar has no need for exact precision"
)]
fn fraction(progress: PieceProgressView) -> f32 {
    if progress.total == 0 {
        return 0.;
    }
    (progress.done as f32 / progress.total as f32).clamp(0., 1.)
}

/// A short `m:ss` clock.
fn clock(value: Duration) -> String {
    let seconds = value.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

/// A download speed in MB/s, as the HUD shows it.
#[expect(
    clippy::as_conversions,
    clippy::cast_precision_loss,
    reason = "a speed shown to one decimal does not need full u64 precision"
)]
pub(crate) fn speed(bytes_per_second: u64) -> String {
    format!("{:.1} MB/s", bytes_per_second as f64 / 1_000_000.)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use gpui_kit::{
        IntoElement, Render, TestAppContext, VisualTestContext, Window, px, test::TestWindowExt,
    };

    use super::{
        BufferOverlay, BufferOverlayActions, BufferOverlayView, BufferStage, CANCEL,
        CHOOSE_ANOTHER, ELAPSED, HEAD_BAR, HEAD_FILL, NO_PEERS, PieceProgressView, TAIL_BAR,
        TAIL_FILL,
    };

    struct OverlayTest {
        view: BufferOverlayView,
        actions: BufferOverlayActions,
    }

    impl Render for OverlayTest {
        fn render(
            &mut self,
            _window: &mut Window,
            _cx: &mut gpui_kit::Context<'_, Self>,
        ) -> impl IntoElement {
            BufferOverlay::new("overlay", self.view.clone(), self.actions.clone())
        }
    }

    fn has_text(cx: &mut VisualTestContext, expected: &str) -> bool {
        let mut found = false;
        cx.update(|window, _| {
            found = gpui_kit::base::test_support::texts(window)
                .iter()
                .any(|text| text.text() == expected);
        });
        found
    }

    fn view() -> BufferOverlayView {
        BufferOverlayView {
            media_title: "A show".into(),
            episode: Some(3),
            episode_title: Some("The beginning".into()),
            stage: BufferStage::Buffering,
            head: PieceProgressView { done: 3, total: 4 },
            tail: PieceProgressView { done: 2, total: 2 },
            peers: 7,
            download_bps: 2_000_000,
            elapsed: Duration::from_secs(12),
            ..BufferOverlayView::default()
        }
    }

    fn open(
        cx: &mut TestAppContext,
        view: BufferOverlayView,
        actions: BufferOverlayActions,
    ) -> &mut VisualTestContext {
        cx.update(gpui_kit::init);
        let (_, cx) = cx.add_window_view(move |_, _| OverlayTest { view, actions });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        cx
    }

    #[gpui_kit::test]
    fn the_buffering_stage_shows_the_trail_windows_and_stats(cx: &mut TestAppContext) {
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&pressed);
        let actions = BufferOverlayActions::new().on_cancel(move |_, _| {
            recorded.borrow_mut().push("cancel");
        });
        let cx = open(cx, view(), actions);

        assert!(has_text(cx, "A show"), "the media title is shown");
        assert!(has_text(cx, "EP 3 · The beginning"));
        assert!(has_text(cx, "metadata"), "the trail names every stage");
        assert!(has_text(cx, "peers"));
        assert!(has_text(cx, "▸ buffering"), "the current stage is marked");
        assert!(has_text(cx, "Head 8 MiB 3/4"), "the head window advances");
        assert!(has_text(cx, "Tail 4 MiB 2/2"), "the tail window advances");
        assert!(
            has_text(cx, "7 peers · 2.0 MB/s"),
            "the stats are formatted"
        );
        assert!(!has_text(cx, "No peers found. Try another release."));

        for selector in [HEAD_BAR, HEAD_FILL, TAIL_BAR, TAIL_FILL] {
            assert!(
                cx.update(|window, _| window.try_find(selector).is_some()),
                "{selector} is drawn"
            );
        }
        assert_eq!(
            cx.debug_bounds(HEAD_FILL).map(|bounds| bounds.size.width),
            Some(px(54.)),
            "the head fill is three quarters of 72 px"
        );

        cx.update(|window, app| window.click(CANCEL, app));
        assert_eq!(*pressed.borrow(), ["cancel"], "Cancel drops the stream");
    }

    #[gpui_kit::test]
    fn metadata_and_peers_mark_their_stage(cx: &mut TestAppContext) {
        let mut metadata = view();
        metadata.stage = BufferStage::Metadata;
        metadata.head = PieceProgressView::default();
        metadata.tail = PieceProgressView::default();
        {
            let visual = open(cx, metadata, BufferOverlayActions::new());
            assert!(has_text(visual, "▸ metadata"));
            assert!(!has_text(visual, "▸ buffering"));
            assert!(
                has_text(visual, "Head 8 MiB"),
                "an empty window has no fraction"
            );
        }

        let mut peers = view();
        peers.stage = BufferStage::Peers;
        let visual = open(cx, peers, BufferOverlayActions::new());
        assert!(has_text(visual, "▸ peers"));
        assert!(!has_text(visual, "▸ metadata"));
    }

    #[gpui_kit::test]
    fn no_peers_offers_another_release_with_an_elapsed_counter(cx: &mut TestAppContext) {
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let cancel = Rc::clone(&pressed);
        let choose = Rc::clone(&pressed);
        let actions = BufferOverlayActions::new()
            .on_cancel(move |_, _| cancel.borrow_mut().push("cancel"))
            .on_choose_another(move |_, _| choose.borrow_mut().push("choose"));
        let mut view = view();
        view.no_peers = true;
        view.elapsed = Duration::from_secs(95);
        let cx = open(cx, view, actions);

        assert!(
            has_text(cx, "No peers found. Try another release."),
            "the amber hint is shown"
        );
        assert!(has_text(cx, "1:35 elapsed"), "the elapsed counter ticks");
        assert!(
            has_text(cx, "7 peers · 2.0 MB/s"),
            "the bar keeps buffering"
        );

        for selector in [NO_PEERS, ELAPSED, CHOOSE_ANOTHER] {
            assert!(
                cx.update(|window, _| window.try_find(selector).is_some()),
                "{selector} is drawn"
            );
        }
        cx.update(|window, app| window.click(CHOOSE_ANOTHER, app));
        cx.update(|window, app| window.click(CANCEL, app));
        assert_eq!(*pressed.borrow(), ["choose", "cancel"]);
    }
}
