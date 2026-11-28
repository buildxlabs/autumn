//! The playback HUD: a presentational bottom bar for the one active session.
//!
//! The view model carries plain data and callbacks from `ui-state`; this
//! widget does no I/O and never names an engine type (ARCHITECTURE §3).

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    App, Bounds, ClickEvent, Entity, ImageSource, InteractiveElement, IntoElement, MouseButton,
    MouseDownEvent, ObjectFit, ParentElement, Pixels, RenderOnce, Resource, SharedString, Styled,
    StyledImage, TestSupportExt, Window,
    assets::IconName,
    base::{Disableable as _, ElementExt, StyledExt as _},
    component::{
        ActiveTheme, Icon, Sizable as _,
        button::{Button, ButtonVariants as _},
        label::Label,
    },
    div, img, px, relative,
};

use crate::buffer_overlay::{amber, speed};

/// Height of the bottom bar.
pub const PLAYBACK_HUD_HEIGHT: f32 = 60.;
/// Thickness of the seek track.
const SEEK_HEIGHT: f32 = 4.;
/// Width of the poster and the time label.
const POSTER_WIDTH: f32 = 36.;
const POSTER_HEIGHT: f32 = 48.;
const CLOCK_WIDTH: f32 = 96.;

const POSTER: &str = "playback-hud-poster";
const POSTER_FALLBACK: &str = "playback-hud-poster-fallback";
const MEDIA_TITLE: &str = "playback-hud-media-title";
const EPISODE_TITLE: &str = "playback-hud-episode-title";
const PLAYER_NAME: &str = "playback-hud-player";
const POSITION: &str = "playback-hud-position";
const SEEK_TRACK: &str = "playback-hud-seek-track";
const SEEK_FILL: &str = "playback-hud-seek-fill";
const PLAY_PAUSE: &str = "playback-hud-play-pause";
const SKIP: &str = "playback-hud-skip";
const NEXT_COUNTDOWN: &str = "playback-hud-next-countdown";
const PLAY_NEXT: &str = "playback-hud-play-next";
const CANCEL_NEXT: &str = "playback-hud-cancel-next";
const BUFFERING_CHIP: &str = "playback-hud-buffering-chip";

/// A chapter the HUD can offer to skip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum SkipKind {
    /// The opening is playing.
    Opening,
    /// The ending is playing.
    Ending,
}

/// A chapter mark on the seek bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChapterMark {
    /// Where the chapter starts, from the beginning of the file.
    pub start: Duration,
}

/// The countdown to the next episode, shown after an episode ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AutoNextView {
    /// The episode that plays at zero.
    pub episode: u32,
    /// Time left before it plays.
    pub remaining: Duration,
}

/// The in-HUD chip for a torrent stream whose relay reads are stalled.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferingView {
    /// Connected peers from the last `Preparing` report.
    pub peers: u32,
    /// Download speed from the last `Preparing` report, in bytes per second.
    pub download_bps: u64,
}

/// Everything the HUD draws, supplied by the store and the shell.
#[derive(Debug, Clone, Default)]
pub struct PlaybackHudView {
    /// The media's poster, when one is known.
    pub poster: Option<Resource>,
    /// The media's title.
    pub media_title: SharedString,
    /// The episode's number in the main run, when known.
    pub episode: Option<u32>,
    /// The episode's title, when known.
    pub episode_title: Option<SharedString>,
    /// The player the engine is driving.
    pub player: SharedString,
    /// Elapsed time.
    pub position: Duration,
    /// The file's duration, when the player knows it.
    pub duration: Option<Duration>,
    /// Whether the player is paused.
    pub paused: bool,
    /// Chapter marks to draw as ticks on the seek bar.
    pub chapters: Vec<ChapterMark>,
    /// Whether to offer Skip Opening or Skip Ending.
    pub skip: Option<SkipKind>,
    /// The next-episode countdown, while one runs.
    pub auto_next: Option<AutoNextView>,
    /// The buffering chip, while the relay reports stalled reads.
    pub buffering: Option<BufferingView>,
}

/// One button callback: the screen updates its store from it.
type HudCallback = Rc<dyn Fn(&mut Window, &mut App)>;
/// The seek callback receives an absolute position in seconds.
type SeekCallback = Rc<dyn Fn(f64, &mut Window, &mut App)>;

/// Callbacks a screen wires to its playback store.
#[derive(Clone, Default)]
pub struct PlaybackHudActions {
    toggle_pause: Option<HudCallback>,
    seek: Option<SeekCallback>,
    skip_back: Option<HudCallback>,
    skip_forward: Option<HudCallback>,
    stop: Option<HudCallback>,
    skip: Option<HudCallback>,
    play_next: Option<HudCallback>,
    cancel_auto_next: Option<HudCallback>,
}

impl std::fmt::Debug for PlaybackHudActions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackHudActions")
            .field("toggle_pause", &self.toggle_pause.is_some())
            .field("seek", &self.seek.is_some())
            .field("skip_back", &self.skip_back.is_some())
            .field("skip_forward", &self.skip_forward.is_some())
            .field("stop", &self.stop.is_some())
            .field("skip", &self.skip.is_some())
            .field("play_next", &self.play_next.is_some())
            .field("cancel_auto_next", &self.cancel_auto_next.is_some())
            .finish()
    }
}

impl PlaybackHudActions {
    /// No callbacks: the bar is presentational until a screen supplies them.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Called when play/pause is pressed.
    #[must_use]
    pub fn on_toggle_pause(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.toggle_pause = Some(Rc::new(action));
        self
    }

    /// Called with the absolute position, in seconds, when the seek bar is clicked.
    #[must_use]
    pub fn on_seek(mut self, action: impl Fn(f64, &mut Window, &mut App) + 'static) -> Self {
        self.seek = Some(Rc::new(action));
        self
    }

    /// Called for the −10 s button.
    #[must_use]
    pub fn on_skip_back(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.skip_back = Some(Rc::new(action));
        self
    }

    /// Called for the +10 s button.
    #[must_use]
    pub fn on_skip_forward(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.skip_forward = Some(Rc::new(action));
        self
    }

    /// Called when Stop is pressed.
    #[must_use]
    pub fn on_stop(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.stop = Some(Rc::new(action));
        self
    }

    /// Called when the Skip Opening / Skip Ending button is pressed.
    #[must_use]
    pub fn on_skip(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.skip = Some(Rc::new(action));
        self
    }

    /// Called when Play now is pressed during the next-episode countdown.
    #[must_use]
    pub fn on_play_next(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.play_next = Some(Rc::new(action));
        self
    }

    /// Called when Cancel is pressed during the next-episode countdown.
    #[must_use]
    pub fn on_cancel_auto_next(mut self, action: impl Fn(&mut Window, &mut App) + 'static) -> Self {
        self.cancel_auto_next = Some(Rc::new(action));
        self
    }
}

/// The layout of the seek track, read when translating a click into a seek.
#[derive(Debug, Default)]
struct SeekState {
    bounds: Option<Bounds<Pixels>>,
}

/// The playback bar element.
#[derive(IntoElement)]
pub struct PlaybackHud {
    id: SharedString,
    view: PlaybackHudView,
    actions: PlaybackHudActions,
    seek: Entity<SeekState>,
}

impl PlaybackHud {
    /// Creates a bar with `id`, `view` and `actions`.
    pub fn new(
        id: impl Into<SharedString>,
        view: PlaybackHudView,
        actions: PlaybackHudActions,
        window: &mut Window,
        cx: &mut App,
    ) -> Self {
        let id = id.into();
        let seek = window.use_keyed_state(id.clone(), cx, |_, _| SeekState::default());
        Self {
            id,
            view,
            actions,
            seek,
        }
    }
}

impl std::fmt::Debug for PlaybackHud {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackHud")
            .field("id", &self.id)
            .field("view", &self.view)
            .field("actions", &self.actions)
            .finish_non_exhaustive()
    }
}

impl RenderOnce for PlaybackHud {
    #[expect(
        clippy::too_many_lines,
        reason = "the bar lays out the poster, titles, seek track and controls in one element tree"
    )]
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let colors = cx.theme().colors;
        let played = fraction(self.view.position, self.view.duration);
        let duration = self.view.duration;
        let seek = self.seek.clone();
        let seek_action = self.actions.seek.clone();
        let title_line = episode_line(&self.view);
        let clock = format!(
            "{} / {}",
            clock(self.view.position),
            duration.map_or_else(|| "--:--".to_owned(), clock)
        );

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
                                    .debug_selector(|| MEDIA_TITLE.into())
                                    .truncate()
                                    .text_sm()
                                    .font_semibold()
                                    .child(Label::new(self.view.media_title.clone())),
                            )
                            .child(
                                div()
                                    .debug_selector(|| EPISODE_TITLE.into())
                                    .truncate()
                                    .text_xs()
                                    .text_color(colors.muted_foreground)
                                    .child(Label::new(title_line)),
                            )
                            .children(self.view.buffering.map(|buffering| {
                                div()
                                    .id(BUFFERING_CHIP)
                                    .test_support()
                                    .debug_selector(|| BUFFERING_CHIP.into())
                                    .flex_none()
                                    .rounded_full()
                                    .bg(amber())
                                    .px_2()
                                    .text_xs()
                                    .text_color(colors.background)
                                    .child(Label::new(format!(
                                        "Buffering · {} peers · {}",
                                        buffering.peers,
                                        speed(buffering.download_bps)
                                    )))
                            })),
                    )
                    .child(
                        div()
                            .id(SEEK_TRACK)
                            .test_support()
                            .debug_selector(|| SEEK_TRACK.into())
                            .relative()
                            .h(px(SEEK_HEIGHT))
                            .w_full()
                            .rounded_full()
                            .bg(colors.border)
                            .on_prepaint({
                                let seek = seek.clone();
                                move |bounds, _, cx| {
                                    if seek.read(cx).bounds != Some(bounds) {
                                        seek.update(cx, |state, _| state.bounds = Some(bounds));
                                    }
                                }
                            })
                            .on_mouse_down(MouseButton::Left, move |event, window, cx| {
                                seek_clicked(
                                    &seek,
                                    seek_action.as_ref(),
                                    duration,
                                    event,
                                    window,
                                    cx,
                                );
                            })
                            .child(
                                div()
                                    .debug_selector(|| SEEK_FILL.into())
                                    .absolute()
                                    .top_0()
                                    .left_0()
                                    .h_full()
                                    .w(relative(played))
                                    .rounded_full()
                                    .bg(colors.accent),
                            )
                            .children(self.view.chapters.iter().map(|chapter| {
                                div()
                                    .absolute()
                                    .top_0()
                                    .left(relative(fraction(chapter.start, duration)))
                                    .h_full()
                                    .w(px(1.))
                                    .bg(colors.foreground.alpha(0.55))
                            })),
                    ),
            )
            .child(
                div()
                    .debug_selector(|| POSITION.into())
                    .w(px(CLOCK_WIDTH))
                    .text_right()
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(Label::new(clock)),
            )
            .child(
                div()
                    .debug_selector(|| PLAYER_NAME.into())
                    .text_xs()
                    .text_color(colors.muted_foreground)
                    .child(Label::new(self.view.player.clone())),
            )
            .children(self.view.auto_next.map(|next| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .debug_selector(|| NEXT_COUNTDOWN.into())
                            .text_xs()
                            .child(Label::new(format!(
                                "EP {} in {}s",
                                next.episode,
                                next.remaining.as_secs()
                            ))),
                    )
                    .child(text_button(
                        PLAY_NEXT,
                        "Play now",
                        self.actions.play_next.as_ref(),
                    ))
                    .child(text_button(
                        CANCEL_NEXT,
                        "Cancel",
                        self.actions.cancel_auto_next.as_ref(),
                    ))
            }))
            .children(self.view.skip.map(|kind| {
                let label = match kind {
                    SkipKind::Opening => "Skip Opening",
                    SkipKind::Ending => "Skip Ending",
                };
                text_button(SKIP, label, self.actions.skip.as_ref())
            }))
            .child(skip_button(
                "playback-hud-back",
                IconName::RotateCcw,
                self.actions.skip_back.as_ref(),
            ))
            .child(play_button(&self.view, self.actions.toggle_pause.as_ref()))
            .child(skip_button(
                "playback-hud-forward",
                IconName::RotateCw,
                self.actions.skip_forward.as_ref(),
            ))
            .child(stop_button(self.actions.stop.as_ref()))
    }
}

/// Translates a click on the track into a seek command.
fn seek_clicked(
    seek: &Entity<SeekState>,
    action: Option<&SeekCallback>,
    duration: Option<Duration>,
    event: &MouseDownEvent,
    window: &mut Window,
    cx: &mut App,
) {
    let (Some(bounds), Some(duration), Some(action)) = (seek.read(cx).bounds, duration, action)
    else {
        return;
    };
    let width = f32::from(bounds.size.width);
    if width <= 0.0 {
        return;
    }
    let x = f32::from(event.position.x - bounds.origin.x);
    let seconds = f64::from((x / width).clamp(0.0, 1.0)) * duration.as_secs_f64();
    action(seconds, window, cx);
}

/// A simple button that invokes one optional callback.
fn action_button(
    id: &'static str,
    icon: IconName,
    action: Option<&HudCallback>,
) -> impl IntoElement {
    let button = Button::new(id)
        .ghost()
        .small()
        .icon(Icon::new(icon).small())
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

fn text_button(
    id: &'static str,
    label: &'static str,
    action: Option<&HudCallback>,
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

fn skip_button(id: &'static str, icon: IconName, action: Option<&HudCallback>) -> impl IntoElement {
    action_button(id, icon, action)
}

fn play_button(view: &PlaybackHudView, action: Option<&HudCallback>) -> impl IntoElement {
    let icon = if view.paused {
        IconName::Play
    } else {
        IconName::Pause
    };
    action_button(PLAY_PAUSE, icon, action)
}

fn stop_button(action: Option<&HudCallback>) -> impl IntoElement {
    action_button("playback-hud-stop", IconName::SquareStop, action)
}

fn poster(view: &PlaybackHudView) -> impl IntoElement {
    match &view.poster {
        Some(poster) => div()
            .debug_selector(|| POSTER.into())
            .flex_none()
            .w(px(POSTER_WIDTH))
            .h(px(POSTER_HEIGHT))
            .rounded_sm()
            .overflow_hidden()
            .child(
                img(ImageSource::Resource(poster.clone()))
                    .size_full()
                    .object_fit(ObjectFit::Cover)
                    .with_fallback(|| poster_fallback().into_any_element()),
            ),
        None => poster_fallback().debug_selector(|| POSTER_FALLBACK.into()),
    }
}

fn poster_fallback() -> gpui_kit::Div {
    div()
        .flex_none()
        .w(px(POSTER_WIDTH))
        .h(px(POSTER_HEIGHT))
        .rounded_sm()
        .bg(gpui_kit::Hsla::default())
}

/// The media title and episode line.
fn episode_line(view: &PlaybackHudView) -> String {
    let number = view.episode.map(|episode| format!("EP {episode}"));
    let title = view.episode_title.as_deref().unwrap_or_default();
    match (number, title.is_empty()) {
        (Some(number), false) => format!("{number} · {title}"),
        (Some(number), true) => number,
        (None, false) => title.to_owned(),
        (None, true) => String::new(),
    }
}

/// Elapsed over duration, clamped to `0..=1`.
fn fraction(position: Duration, duration: Option<Duration>) -> f32 {
    let Some(duration) = duration.filter(|duration| !duration.is_zero()) else {
        return 0.;
    };
    (position.as_secs_f32() / duration.as_secs_f32()).clamp(0., 1.)
}

/// A short `m:ss` clock.
fn clock(value: Duration) -> String {
    let seconds = value.as_secs();
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use gpui_kit::{
        Context, IntoElement, Render, TestAppContext, VisualTestContext, Window, point, px,
        test::{ClickOptions, TestWindowExt},
    };

    use super::{
        AutoNextView, BUFFERING_CHIP, BufferingView, CANCEL_NEXT, ChapterMark, PLAY_NEXT,
        PlaybackHud, PlaybackHudActions, PlaybackHudView, SEEK_TRACK, SKIP, SkipKind,
    };

    struct HudTest {
        view: PlaybackHudView,
        actions: PlaybackHudActions,
    }

    impl Render for HudTest {
        fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
            PlaybackHud::new("hud", self.view.clone(), self.actions.clone(), window, cx)
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

    fn view() -> PlaybackHudView {
        PlaybackHudView {
            media_title: "A show".into(),
            episode: Some(3),
            episode_title: Some("The beginning".into()),
            player: "mpv".into(),
            position: Duration::from_secs(60),
            duration: Some(Duration::from_secs(240)),
            chapters: vec![
                ChapterMark {
                    start: Duration::from_secs(20),
                },
                ChapterMark {
                    start: Duration::from_secs(200),
                },
            ],
            ..PlaybackHudView::default()
        }
    }

    fn open(
        cx: &mut TestAppContext,
        view: PlaybackHudView,
    ) -> (Rc<RefCell<Vec<f64>>>, &mut VisualTestContext) {
        cx.update(gpui_kit::init);
        let seeks = Rc::new(RefCell::new(Vec::new()));
        let recorded = Rc::clone(&seeks);
        let actions = PlaybackHudActions::new().on_seek(move |seconds, _, _| {
            recorded.borrow_mut().push(seconds);
        });
        let (_, cx) = cx.add_window_view(move |_, _| HudTest { view, actions });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        (seeks, cx)
    }

    #[gpui_kit::test]
    fn renders_the_position_and_chapter_ticks(cx: &mut TestAppContext) {
        let (_, cx) = open(cx, view());
        assert!(has_text(cx, "1:00 / 4:00"), "the clock follows the view");
        assert!(has_text(cx, "A show"), "the media title is shown");
        assert!(has_text(cx, "EP 3 · The beginning"));
        assert!(has_text(cx, "mpv"));
        assert_eq!(
            cx.debug_bounds(SEEK_TRACK).map(|b| b.size.height),
            Some(px(4.))
        );
    }

    #[gpui_kit::test]
    fn the_skip_button_shows_the_chapter_kind_and_fires(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let skips = Rc::new(RefCell::new(0));
        let recorded = Rc::clone(&skips);
        let actions = PlaybackHudActions::new().on_skip(move |_, _| {
            *recorded.borrow_mut() += 1;
        });
        let mut view = view();
        view.skip = Some(SkipKind::Opening);
        let (_, cx) = cx.add_window_view(move |_, _| HudTest { view, actions });
        cx.update(|window, cx| window.draw(cx).clear(cx));
        assert!(
            cx.update(|window, _| window.try_find(SKIP).is_some()),
            "the skip button is drawn"
        );
        assert!(has_text(cx, "Skip Opening"), "the button names the chapter");

        cx.update(|window, app| window.click(SKIP, app));
        assert_eq!(*skips.borrow(), 1, "the skip callback fired");
    }

    #[gpui_kit::test]
    fn the_countdown_offers_play_now_and_cancel(cx: &mut TestAppContext) {
        cx.update(gpui_kit::init);
        let pressed = Rc::new(RefCell::new(Vec::new()));
        let (now, cancel) = (Rc::clone(&pressed), Rc::clone(&pressed));
        let actions = PlaybackHudActions::new()
            .on_play_next(move |_, _| now.borrow_mut().push("play now"))
            .on_cancel_auto_next(move |_, _| cancel.borrow_mut().push("cancel"));
        let mut view = view();
        view.auto_next = Some(AutoNextView {
            episode: 4,
            remaining: Duration::from_secs(5),
        });
        let (_, cx) = cx.add_window_view(move |_, _| HudTest { view, actions });
        cx.update(|window, cx| window.draw(cx).clear(cx));

        assert!(
            has_text(cx, "EP 4 in 5s"),
            "the countdown names the episode"
        );
        cx.update(|window, app| window.click(PLAY_NEXT, app));
        cx.update(|window, app| window.click(CANCEL_NEXT, app));
        assert_eq!(*pressed.borrow(), ["play now", "cancel"]);
    }

    #[gpui_kit::test]
    fn the_buffering_chip_shows_peers_and_speed_until_reads_flow(cx: &mut TestAppContext) {
        let mut buffering = view();
        buffering.buffering = Some(BufferingView {
            peers: 4,
            download_bps: 1_500_000,
        });
        {
            let (_, cx) = open(cx, buffering);
            assert!(
                has_text(cx, "Buffering · 4 peers · 1.5 MB/s"),
                "the chip follows the stream stats"
            );
            assert!(
                cx.update(|window, _| window.try_find(BUFFERING_CHIP).is_some()),
                "the chip is drawn next to the title"
            );
        }

        let (_, cx) = open(cx, view());
        assert!(
            cx.update(|window, _| window.try_find(BUFFERING_CHIP).is_none()),
            "the chip disappears when reads unblock"
        );
    }

    #[gpui_kit::test]
    fn clicking_the_seek_bar_sends_an_absolute_position(cx: &mut TestAppContext) {
        let (seeks, cx) = open(cx, view());
        cx.update(|window, app| {
            window.click_with_options(
                SEEK_TRACK,
                ClickOptions::new().with_offset(point(px(0.), px(2.))),
                app,
            );
        });
        assert_eq!(seeks.borrow().as_slice(), &[0.0], "the left edge");

        let width = cx.debug_bounds(SEEK_TRACK).expect("the track").size.width;
        cx.update(|window, app| {
            window.click_with_options(
                SEEK_TRACK,
                ClickOptions::new().with_offset(point(width / 2., px(2.))),
                app,
            );
        });
        assert_eq!(seeks.borrow().as_slice(), &[0.0, 120.0]);
    }
}
