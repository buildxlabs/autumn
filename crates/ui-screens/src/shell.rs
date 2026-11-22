//! The window's root view: titlebar, rail and route view. See ARCHITECTURE §5.

use gpui_kit::{
    App, Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString, Styled,
    Window,
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, TitleBar,
        button::{Button, ButtonVariants},
    },
    div, px,
};
use ui_components::{Rail, RailItem};
use ui_state::Route;

use crate::placeholder::placeholder;

/// Height of the transparent titlebar; macOS traffic lights are centred in it.
const TITLEBAR_HEIGHT: f32 = 40.;

/// Rail entries, top to bottom.
const RAIL_ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];

/// The shell around every route: titlebar on top, rail on the left.
#[derive(Debug)]
pub struct AppShell {
    route: Route,
}

impl AppShell {
    /// A shell showing the home route.
    pub fn new() -> Self {
        Self { route: Route::Home }
    }

    /// The route on screen.
    pub fn route(&self) -> Route {
        self.route
    }

    fn navigate(&mut self, route: Route, cx: &mut Context<'_, Self>) {
        if self.route != route {
            self.route = route;
            cx.notify();
        }
    }

    fn render_titlebar(cx: &App) -> impl IntoElement {
        // Back and forward stay disabled until the router exists (#40).
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
                            .disabled(true),
                    )
                    .child(
                        Button::new("forward")
                            .ghost()
                            .small()
                            .icon(IconName::ChevronRight)
                            .disabled(true),
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
        let items = RAIL_ROUTES
            .iter()
            .map(|&route| RailItem {
                id: rail_id(route).into(),
                label: title(route),
                icon: icon(route),
                active: route == self.route,
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

impl Default for AppShell {
    fn default() -> Self {
        Self::new()
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        div()
            .id("shell")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.background)
            .text_color(colors.foreground)
            .child(Self::render_titlebar(cx))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(self.render_rail(cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .child(placeholder(title(self.route), colors.muted_foreground)),
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
        AppContext, Bounds, Entity, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
        base::Root, point, px, size, test::TestWindowExt,
    };
    use ui_state::Route;

    use super::AppShell;

    fn open_shell(cx: &mut TestAppContext) -> (WindowHandle<Root>, Entity<AppShell>) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ui_theme::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(1280.), px(800.)),
                })),
                ..WindowOptions::default()
            };
            let (window, shell) =
                gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| AppShell::new())).unwrap();
            (window.downcast::<Root>().unwrap(), shell)
        })
    }

    fn click(cx: &mut TestAppContext, window: WindowHandle<Root>, id: &'static str) {
        cx.update_window(window.into(), |_, window, cx| {
            window.render_frame(cx);
            window.click(id, cx);
        })
        .unwrap();
    }

    #[gpui_kit::test]
    fn rail_clicks_switch_the_route(cx: &mut TestAppContext) {
        let (window, shell) = open_shell(cx);
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.route()),
            Route::Home,
            "starts at home"
        );

        click(cx, window, "rail-discover");
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.route()),
            Route::Discover,
            "after discover"
        );

        click(cx, window, "rail-schedule");
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.route()),
            Route::Schedule,
            "after schedule"
        );

        click(cx, window, "rail-home");
        assert_eq!(
            shell.read_with(cx, |shell, _| shell.route()),
            Route::Home,
            "back home"
        );
    }
}
