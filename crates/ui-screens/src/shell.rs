//! The window's root view: titlebar, rail and route view. See ARCHITECTURE §5.

use gpui_kit::{
    AppContext, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, NavigationDirection, ParentElement, Pixels, Point, Render, ScrollHandle,
    SharedString, StatefulInteractiveElement, Styled, Subscription, TestSupportExt, Window,
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, TitleBar, WindowExt,
        button::{Button, ButtonVariants},
    },
    div, px,
};
use ui_components::{Rail, RailItem};
use ui_state::{
    Route, RouterState,
    actions::{GoBack, GoForward, OpenCommandPalette, OpenSearch, WORKSPACE},
};

use crate::{
    palette::{CommandPalette, Mode, PaletteEvent},
    placeholder::placeholder,
};

/// Height of the transparent titlebar; macOS traffic lights are centred in it.
const TITLEBAR_HEIGHT: f32 = 40.;

/// Width of the command palette dialog.
const PALETTE_WIDTH: f32 = 560.;

/// Rail entries, top to bottom.
const RAIL_ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];

/// The shell around every route: titlebar on top, rail on the left.
///
/// It owns the navigation history and the route view's scroll handle, so
/// leaving a route saves its offset and coming back restores it. It also
/// hosts the command palette.
pub struct AppShell {
    router: RouterState,
    scroll: ScrollHandle,
    focus: FocusHandle,
    palette: Entity<CommandPalette>,
    _palette_events: Subscription,
}

impl std::fmt::Debug for AppShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppShell")
            .field("router", &self.router)
            .finish_non_exhaustive()
    }
}

impl AppShell {
    /// A shell showing the home route, focused so its key bindings apply.
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
        let palette_events = cx.subscribe_in(
            &palette,
            window,
            |shell, _, event: &PaletteEvent, window, cx| match *event {
                PaletteEvent::Navigate(route) => {
                    window.close_dialog(cx);
                    shell.navigate(route, cx);
                }
            },
        );
        Self {
            router: RouterState::new(Route::Home),
            scroll: ScrollHandle::new(),
            focus,
            palette,
            _palette_events: palette_events,
        }
    }

    /// The route on screen.
    pub fn route(&self) -> Route {
        self.router.current()
    }

    fn navigate(&mut self, route: Route, cx: &mut Context<'_, Self>) {
        let restore = self.router.push(route, self.scroll.offset());
        self.show(restore, cx);
    }

    /// Opens the command palette in `mode`, unless a dialog is already up.
    fn open_palette(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<'_, Self>) {
        if window.has_active_dialog(cx) {
            return;
        }
        let palette = self.palette.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(PALETTE_WIDTH))
                .close_button(false)
                .child(palette.clone())
        });
        // After the dialog took focus, so the query field ends up with it.
        self.palette
            .update(cx, |palette, cx| palette.open(mode, window, cx));
    }

    fn open_command_palette(
        &mut self,
        _: &OpenCommandPalette,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        self.open_palette(Mode::Navigate, window, cx);
    }

    fn open_search(&mut self, _: &OpenSearch, window: &mut Window, cx: &mut Context<'_, Self>) {
        self.open_palette(Mode::Search, window, cx);
    }

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<'_, Self>) {
        let restore = self.router.back(self.scroll.offset());
        self.show(restore, cx);
    }

    fn go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<'_, Self>) {
        let restore = self.router.forward(self.scroll.offset());
        self.show(restore, cx);
    }

    /// Scrolls the new route to its saved offset, if the route changed.
    ///
    /// Layout clamps the offset to the content drawn in the next frame, so a
    /// screen whose content loads asynchronously must reapply it once loaded.
    fn show(&mut self, restore: Option<Point<Pixels>>, cx: &mut Context<'_, Self>) {
        if let Some(offset) = restore {
            self.scroll.set_offset(offset);
            cx.notify();
        }
    }

    fn render_titlebar(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        TitleBar::new()
            .h(px(TITLEBAR_HEIGHT))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        Button::new("back")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronLeft)
                            .disabled(!self.router.can_go_back())
                            .on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                shell.go_back(&GoBack, window, cx);
                            })),
                    )
                    .child(
                        Button::new("forward")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronRight)
                            .disabled(!self.router.can_go_forward())
                            .on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
                                shell.go_forward(&GoForward, window, cx);
                            })),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .justify_center()
                    .child(search_pill(cx)),
            )
    }

    fn render_rail(&self, cx: &mut Context<'_, Self>) -> Rail {
        let current = self.router.current();
        let items = RAIL_ROUTES
            .iter()
            .map(|&route| RailItem {
                id: rail_id(route).into(),
                label: title(route),
                icon: icon(route),
                active: route == current,
            })
            .collect();
        let shell = cx.entity().downgrade();
        Rail::new(items, move |index, _, cx| {
            if let Some(&route) = RAIL_ROUTES.get(index) {
                // The shell outlives its rail, so the upgrade fails only
                // while the window is closing.
                if let Some(shell) = shell.upgrade() {
                    shell.update(cx, |shell, cx| shell.navigate(route, cx));
                }
            }
        })
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let route = self.router.current();
        div()
            .id("shell")
            .track_focus(&self.focus)
            .key_context(WORKSPACE)
            .on_action(cx.listener(Self::go_back))
            .on_action(cx.listener(Self::go_forward))
            .on_action(cx.listener(Self::open_command_palette))
            .on_action(cx.listener(Self::open_search))
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Back),
                cx.listener(|shell, _, window, cx| shell.go_back(&GoBack, window, cx)),
            )
            .on_mouse_down(
                MouseButton::Navigate(NavigationDirection::Forward),
                cx.listener(|shell, _, window, cx| shell.go_forward(&GoForward, window, cx)),
            )
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(self.render_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_rail(cx))
                    .child(
                        div()
                            .id("route-view")
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.scroll)
                            .child(placeholder(
                                title(route),
                                rows(route),
                                colors.muted_foreground,
                            ))
                            .test_support(),
                    ),
            )
    }
}

/// The search pill; a click opens the command palette.
fn search_pill(cx: &mut Context<'_, AppShell>) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id("search-pill")
        .w(px(320.))
        .h(px(28.))
        .px_3()
        .flex()
        .items_center()
        .gap_2()
        .rounded_full()
        .bg(theme.sidebar)
        .border_1()
        .border_color(theme.border)
        .text_sm()
        .text_color(theme.muted_foreground)
        .child(Icon::new(IconName::Search).small())
        .child("Search")
        .cursor_pointer()
        .on_click(cx.listener(|shell, _: &ClickEvent, window, cx| {
            shell.open_palette(Mode::Navigate, window, cx);
        }))
        .test_support()
}

fn rail_id(route: Route) -> &'static str {
    match route {
        Route::Home => "rail-home",
        Route::Discover => "rail-discover",
        Route::Schedule => "rail-schedule",
        Route::Manga => "rail-manga",
    }
}

pub(crate) fn title(route: Route) -> SharedString {
    match route {
        Route::Home => "Home",
        Route::Discover => "Discover",
        Route::Schedule => "Schedule",
        Route::Manga => "Manga",
    }
    .into()
}

/// Discover gets a long list so scroll restore can be tried by hand.
fn rows(route: Route) -> usize {
    match route {
        Route::Discover => 100,
        Route::Home | Route::Schedule | Route::Manga => 0,
    }
}

pub(crate) fn icon(route: Route) -> IconName {
    match route {
        Route::Home => IconName::House,
        Route::Discover => IconName::Compass,
        Route::Schedule => IconName::Calendar,
        Route::Manga => IconName::BookOpen,
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext, Bounds, Entity, MouseButton, NavigationDirection, Pixels, Point, ScrollDelta,
        TestAppContext, WindowBounds, WindowHandle, WindowOptions,
        base::Root,
        component::WindowExt,
        point, px, size,
        test::{ClickOptions, TestWindowExt},
    };
    use ui_state::{Route, actions::OpenCommandPalette};

    use super::AppShell;
    use crate::palette::Mode;

    // Must match `NAVIGATION_KEYS` in ui-state's actions.rs.
    #[cfg(target_os = "macos")]
    const BACK_KEY: &str = "cmd-[";
    #[cfg(not(target_os = "macos"))]
    const BACK_KEY: &str = "ctrl-[";
    #[cfg(target_os = "macos")]
    const FORWARD_KEY: &str = "cmd-]";
    #[cfg(not(target_os = "macos"))]
    const FORWARD_KEY: &str = "ctrl-]";
    // Must match `PALETTE_KEY` in ui-state's actions.rs.
    #[cfg(target_os = "macos")]
    const PALETTE_KEY: &str = "cmd-j";
    #[cfg(not(target_os = "macos"))]
    const PALETTE_KEY: &str = "ctrl-j";

    struct Shell {
        window: WindowHandle<Root>,
        view: Entity<AppShell>,
    }

    impl Shell {
        fn open(cx: &mut TestAppContext) -> Self {
            let (window, view) = cx.update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                ui_state::actions::bind_keys(cx);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(800.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| AppShell::new(window, cx))
                })
                .unwrap()
            });
            Self {
                window: window.downcast::<Root>().unwrap(),
                view,
            }
        }

        fn act(
            &self,
            cx: &mut TestAppContext,
            action: impl FnOnce(&mut gpui_kit::Window, &mut gpui_kit::App),
        ) {
            cx.update_window(self.window.into(), |_, window, cx| {
                window.render_frame(cx);
                action(window, cx);
                window.render_frame(cx);
            })
            .unwrap();
        }

        fn click(&self, cx: &mut TestAppContext, id: &'static str) {
            self.act(cx, |window, cx| window.click(id, cx));
        }

        fn press(&self, cx: &mut TestAppContext, key: &'static str) {
            self.act(cx, |window, cx| window.press(key, cx));
        }

        fn mouse(&self, cx: &mut TestAppContext, direction: NavigationDirection) {
            let options = ClickOptions::new().with_button(MouseButton::Navigate(direction));
            self.act(cx, |window, cx| {
                window.click_with_options("route-view", options, cx);
            });
        }

        fn route(&self, cx: &mut TestAppContext) -> Route {
            self.view.read_with(cx, |shell, _| shell.route())
        }

        fn palette_open(&self, cx: &mut TestAppContext) -> bool {
            cx.update_window(self.window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.has_active_dialog(cx)
            })
            .unwrap()
        }

        fn input(&self, cx: &mut TestAppContext, text: &'static str) {
            self.act(cx, |window, cx| window.input(text, cx));
            cx.run_until_parked();
        }

        fn query(&self, cx: &mut TestAppContext) -> String {
            self.view
                .read_with(cx, |shell, cx| shell.palette.read(cx).query(cx).to_string())
        }

        fn mode(&self, cx: &mut TestAppContext) -> Mode {
            self.view
                .read_with(cx, |shell, cx| shell.palette.read(cx).mode(cx))
        }

        fn offset(&self, cx: &mut TestAppContext) -> Point<Pixels> {
            self.view.read_with(cx, |shell, _| shell.scroll.offset())
        }
    }

    #[gpui_kit::test]
    fn rail_clicks_switch_the_route(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        assert_eq!(shell.route(cx), Route::Home, "starts at home");

        shell.click(cx, "rail-discover");
        assert_eq!(shell.route(cx), Route::Discover, "after discover");

        shell.click(cx, "rail-schedule");
        assert_eq!(shell.route(cx), Route::Schedule, "after schedule");

        shell.click(cx, "rail-home");
        assert_eq!(shell.route(cx), Route::Home, "back home");
    }

    /// The Phase 0 exit criterion.
    #[gpui_kit::test]
    fn back_restores_the_scroll_position(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-discover");
        shell.act(cx, |window, cx| {
            window.scroll(
                "route-view",
                ScrollDelta::Pixels(point(px(0.), px(-600.))),
                cx,
            );
        });
        let scrolled = shell.offset(cx);
        assert!(
            scrolled.y < px(0.),
            "the list scrolled down, got {scrolled:?}"
        );

        shell.click(cx, "rail-home");
        assert_eq!(shell.offset(cx), Point::default(), "home opens at the top");

        shell.press(cx, BACK_KEY);
        assert_eq!(shell.route(cx), Route::Discover, "back returns to discover");
        assert_eq!(shell.offset(cx), scrolled, "discover is where it was left");
    }

    #[gpui_kit::test]
    fn titlebar_keys_and_mouse_buttons_walk_the_history(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "back");
        assert_eq!(
            shell.route(cx),
            Route::Home,
            "back is disabled with no history"
        );

        shell.click(cx, "rail-discover");
        shell.click(cx, "back");
        assert_eq!(shell.route(cx), Route::Home, "titlebar back");
        shell.click(cx, "forward");
        assert_eq!(shell.route(cx), Route::Discover, "titlebar forward");

        shell.press(cx, BACK_KEY);
        assert_eq!(shell.route(cx), Route::Home, "back key");
        shell.press(cx, FORWARD_KEY);
        assert_eq!(shell.route(cx), Route::Discover, "forward key");

        shell.mouse(cx, NavigationDirection::Back);
        assert_eq!(shell.route(cx), Route::Home, "mouse back button");
        shell.mouse(cx, NavigationDirection::Forward);
        assert_eq!(shell.route(cx), Route::Discover, "mouse forward button");
    }

    #[gpui_kit::test]
    fn the_palette_key_opens_the_palette_and_escape_closes_it(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        assert!(!shell.palette_open(cx), "closed at start");

        shell.press(cx, PALETTE_KEY);
        assert!(shell.palette_open(cx), "the shortcut opens it");

        shell.press(cx, "escape");
        assert!(!shell.palette_open(cx), "escape closes it");
    }

    #[gpui_kit::test]
    fn q_s_and_the_search_pill_open_the_palette(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);

        shell.press(cx, "q");
        assert!(shell.palette_open(cx), "q opens it");
        assert_eq!(shell.mode(cx), Mode::Navigate, "q opens it to navigate");
        shell.press(cx, "escape");

        shell.press(cx, "s");
        assert!(shell.palette_open(cx), "s opens it");
        assert_eq!(shell.query(cx), "/search ", "s opens it in /search mode");
        assert_eq!(shell.mode(cx), Mode::Search, "in search mode");
        shell.press(cx, "escape");
        assert!(!shell.palette_open(cx), "one escape closes a bare /search");

        shell.click(cx, "search-pill");
        assert!(shell.palette_open(cx), "the pill opens it");
        assert_eq!(shell.query(cx), "", "reopens with an empty query");
    }

    #[gpui_kit::test]
    fn picking_a_route_navigates_and_closes(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.act(cx, |window, cx| {
            window.dispatch_action(Box::new(OpenCommandPalette), cx);
        });
        assert!(shell.palette_open(cx), "the action opens it");

        shell.input(cx, "disc");
        shell.press(cx, "enter");
        cx.run_until_parked();

        assert_eq!(shell.route(cx), Route::Discover, "navigated to discover");
        assert!(!shell.palette_open(cx), "the palette closed");

        shell.press(cx, "q");
        assert!(shell.palette_open(cx), "focus is back on the shell");
    }

    #[gpui_kit::test]
    fn slash_commands_switch_the_mode(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.press(cx, "q");

        shell.input(cx, "/anime");
        assert_eq!(shell.mode(cx), Mode::Anime, "typing /anime switches");
        shell.act(cx, |window, _| {
            assert!(window.find("palette-empty").visible(), "empty state shown");
        });
        // With text typed, escape clears the query and the palette stays.
        shell.input(cx, " naruto");
        assert_eq!(shell.mode(cx), Mode::Anime, "still anime with text");
        shell.press(cx, "escape");
        assert!(shell.palette_open(cx), "still open");
        assert_eq!(shell.query(cx), "", "query cleared");

        shell.input(cx, "/ma");
        shell.press(cx, "enter");
        cx.run_until_parked();
        assert_eq!(shell.query(cx), "/manga ", "picking /manga fills it in");
        assert_eq!(shell.mode(cx), Mode::Manga, "in manga mode");
        assert_eq!(shell.route(cx), Route::Home, "no navigation");
    }
}
