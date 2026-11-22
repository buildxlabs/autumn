//! The window's root view: titlebar, rail and route view. See ARCHITECTURE §5.

use gpui_kit::{
    App, ClickEvent, Context, FocusHandle, InteractiveElement, IntoElement, MouseButton,
    NavigationDirection, ParentElement, Pixels, Point, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, TestSupportExt, Window,
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, TitleBar,
        button::{Button, ButtonVariants},
    },
    div, px,
};
use ui_components::{Rail, RailItem};
use ui_state::{
    Route, RouterState,
    actions::{GoBack, GoForward, WORKSPACE},
};

use crate::placeholder::placeholder;

/// Height of the transparent titlebar; macOS traffic lights are centred in it.
const TITLEBAR_HEIGHT: f32 = 40.;

/// Rail entries, top to bottom.
const RAIL_ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];

/// The shell around every route: titlebar on top, rail on the left.
///
/// It owns the navigation history and the route view's scroll handle, so
/// leaving a route saves its offset and coming back restores it.
#[derive(Debug)]
pub struct AppShell {
    router: RouterState,
    scroll: ScrollHandle,
    focus: FocusHandle,
}

impl AppShell {
    /// A shell showing the home route, focused so its key bindings apply.
    pub fn new(window: &mut Window, cx: &mut Context<'_, Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        Self {
            router: RouterState::new(Route::Home),
            scroll: ScrollHandle::new(),
            focus,
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

/// The search pill; it opens the command palette once that exists (Phase 1).
fn search_pill(cx: &App) -> impl IntoElement {
    let theme = cx.theme();
    div()
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
}

fn rail_id(route: Route) -> &'static str {
    match route {
        Route::Home => "rail-home",
        Route::Discover => "rail-discover",
        Route::Schedule => "rail-schedule",
        Route::Manga => "rail-manga",
    }
}

fn title(route: Route) -> SharedString {
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

fn icon(route: Route) -> IconName {
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
        point, px, size,
        test::{ClickOptions, TestWindowExt},
    };
    use ui_state::Route;

    use super::AppShell;

    // Must match `NAVIGATION_KEYS` in ui-state's actions.rs.
    #[cfg(target_os = "macos")]
    const BACK_KEY: &str = "cmd-[";
    #[cfg(not(target_os = "macos"))]
    const BACK_KEY: &str = "ctrl-[";
    #[cfg(target_os = "macos")]
    const FORWARD_KEY: &str = "cmd-]";
    #[cfg(not(target_os = "macos"))]
    const FORWARD_KEY: &str = "ctrl-]";

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
}
