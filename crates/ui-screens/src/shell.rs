//! The window's root view: titlebar, rail and route view. See ARCHITECTURE §5.

use std::{rc::Rc, time::Duration};

use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, NavigationDirection, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt, Window,
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, TitleBar, WindowExt,
        button::{Button, ButtonVariants},
        switch::Switch,
    },
    div, px,
};
use ui_components::{RAIL_WIDTH, Rail, RailExpansion, RailItem};
use ui_state::{
    RailBadges, Route, RouterState, ScrollRestore,
    actions::{GoBack, GoForward, OpenCommandPalette, OpenSearch, WORKSPACE},
};

use crate::{
    palette::{CommandPalette, Mode, PaletteEvent},
    placeholder::{placeholder, poster_grid},
};

/// Height of the transparent titlebar; macOS traffic lights are centred in it.
const TITLEBAR_HEIGHT: f32 = 40.;

/// Width of the command palette dialog.
const PALETTE_WIDTH: f32 = 560.;

/// How long the pointer rests on the rail before it expands (decision #19).
pub(crate) const HOVER_OPEN_DELAY: Duration = Duration::from_millis(150);

/// Rail entries from the top, top to bottom.
const RAIL_ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];
/// The system group at the rail's bottom, top to bottom.
/// The rail's system group, which never folds into "More".
pub const SYSTEM_ROUTES: [Route; 3] = [Route::Extensions, Route::Sync, Route::Settings];

/// The rail preferences the user can change, loaded at start and saved on
/// every change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct RailPrefs {
    /// How the rail opens.
    pub mode: RailMode,
    /// Main items the user moved into "More".
    pub unpinned: Vec<Route>,
}

impl RailPrefs {
    /// Preferences with the rail opening in `mode` and `unpinned` in "More".
    pub fn new(mode: RailMode, unpinned: Vec<Route>) -> Self {
        Self { mode, unpinned }
    }
}

/// How the rail opens (decision #19).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum RailMode {
    /// It opens on hover, over the content.
    #[default]
    Hover,
    /// A toggle button opens it, and the open rail pushes the content aside.
    ClickPin,
}

/// Saves changed preferences. The shell holds the returned task, so a newer
/// save drops an older one still running.
type SavePrefs = Rc<dyn Fn(&RailPrefs, &mut App) -> Task<()>>;

/// The shell around every route: titlebar on top, rail on the left.
///
/// It owns the navigation history and the route view's scroll handle, so
/// leaving a route saves its offset and coming back restores it. It also
/// hosts the command palette.
pub struct AppShell {
    router: RouterState<Route>,
    /// Async screens emit `ContentLoaded`; subscribe the shell to call
    /// `scroll.content_loaded()` so their saved offset survives the load.
    scroll: ScrollRestore,
    focus: FocusHandle,
    palette: Entity<CommandPalette>,
    _palette_events: Subscription,
    prefs: RailPrefs,
    save_prefs: SavePrefs,
    /// The latest save, if it is still running.
    saving: Option<Task<()>>,
    /// Whether the rail is expanded, by hover or by the click-pin toggle.
    rail_open: bool,
    /// The pending hover open; dropping it cancels the open.
    hover_open: Option<Task<()>>,
    /// Whether the pointer is on the rail.
    rail_hovered: bool,
    /// How many of the rail's menus are open. The pointer leaves the rail
    /// for them, so a hover-opened rail waits for them to close.
    rail_menus: usize,
    /// The counts on the rail's items.
    badges: Entity<RailBadges>,
}

impl std::fmt::Debug for AppShell {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppShell")
            .field("router", &self.router)
            .field("prefs", &self.prefs)
            .field("rail_open", &self.rail_open)
            .finish_non_exhaustive()
    }
}

impl AppShell {
    /// A shell showing the home route, focused so its key bindings apply.
    /// `save_prefs` runs whenever the user changes `prefs`.
    pub fn new(
        prefs: RailPrefs,
        save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
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
        let badges = cx.new(|_| RailBadges::placeholder());
        cx.observe(&badges, |_, _, cx| cx.notify()).detach();
        Self {
            router: RouterState::new(Route::Home),
            scroll: ScrollRestore::new(),
            focus,
            palette,
            _palette_events: palette_events,
            prefs,
            save_prefs: Rc::new(save_prefs),
            saving: None,
            rail_open: false,
            hover_open: None,
            rail_hovered: false,
            rail_menus: 0,
            badges,
        }
    }

    /// The route on screen.
    pub fn route(&self) -> Route {
        *self.router.current()
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
    /// [`ScrollRestore`] keeps the offset so an async screen's
    /// `ContentLoaded` can apply it again after layout clamps it.
    fn show(&mut self, restore: Option<Point<Pixels>>, cx: &mut Context<'_, Self>) {
        if let Some(offset) = restore {
            self.scroll.restore(offset);
            cx.notify();
        }
    }

    /// Hover mode: opens the rail once the pointer has rested on it for
    /// [`HOVER_OPEN_DELAY`].
    fn rail_entered(&mut self, cx: &mut Context<'_, Self>) {
        self.rail_hovered = true;
        if self.prefs.mode != RailMode::Hover {
            return;
        }
        self.hover_open = Some(cx.spawn(async move |shell, cx| {
            cx.background_executor().timer(HOVER_OPEN_DELAY).await;
            // Fails only if the shell is gone, and then there is no rail.
            let _ = shell.update(cx, AppShell::open_rail);
        }));
    }

    /// Hover mode: closes the rail as soon as the pointer leaves it, unless
    /// one of its menus is open.
    fn rail_left(&mut self, cx: &mut Context<'_, Self>) {
        self.rail_hovered = false;
        self.close_hover_rail(cx);
    }

    /// Counts a rail menu opening; the pointer leaves the rail for it.
    fn rail_menu_opened(&mut self) {
        self.rail_menus += 1;
    }

    /// Counts a rail menu closing. The last one closing with the pointer off
    /// the rail closes a hover-opened rail.
    fn rail_menu_closed(&mut self, cx: &mut Context<'_, Self>) {
        self.rail_menus = self.rail_menus.saturating_sub(1);
        self.close_hover_rail(cx);
    }

    fn close_hover_rail(&mut self, cx: &mut Context<'_, Self>) {
        if self.prefs.mode == RailMode::Hover && !self.rail_hovered && self.rail_menus == 0 {
            self.close_rail(cx);
        }
    }

    fn open_rail(&mut self, cx: &mut Context<'_, Self>) {
        self.hover_open = None;
        if !self.rail_open {
            self.rail_open = true;
            cx.notify();
        }
    }

    /// Closes the rail and drops a pending hover open.
    fn close_rail(&mut self, cx: &mut Context<'_, Self>) {
        self.hover_open = None;
        if self.rail_open {
            self.rail_open = false;
            cx.notify();
        }
    }

    fn set_rail_mode(&mut self, mode: RailMode, cx: &mut Context<'_, Self>) {
        if self.prefs.mode == mode {
            return;
        }
        self.prefs.mode = mode;
        // Each mode starts collapsed.
        self.close_rail(cx);
        self.saving = Some((self.save_prefs)(&self.prefs, cx));
        cx.notify();
    }

    /// Moves `route` into "More", or back onto the rail, and saves that.
    fn toggle_pinned(&mut self, route: Route, cx: &mut Context<'_, Self>) {
        if let Some(at) = self.prefs.unpinned.iter().position(|&r| r == route) {
            self.prefs.unpinned.remove(at);
        } else {
            self.prefs.unpinned.push(route);
        }
        self.saving = Some((self.save_prefs)(&self.prefs, cx));
        cx.notify();
    }

    fn rail_expansion(&self) -> RailExpansion {
        match (self.rail_open, self.prefs.mode) {
            (false, _) => RailExpansion::Collapsed,
            (true, RailMode::Hover) => RailExpansion::Overlay,
            (true, RailMode::ClickPin) => RailExpansion::Pushed,
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
        let current = *self.router.current();
        let badges = self.badges.read(cx);
        let item = |route: Route| RailItem {
            id: rail_id(route).into(),
            label: title(route),
            icon: icon(route),
            active: route == current,
            badge: badges.count(route),
            pinned: !self.prefs.unpinned.contains(&route),
        };
        let items = RAIL_ROUTES.map(item).into();
        let system = SYSTEM_ROUTES.map(item).into();
        let shell = cx.entity().downgrade();
        let pin_shell = shell.clone();
        let menu_shell = shell.clone();
        let rail = Rail::new(items, system, "Guest", move |id, _, cx| {
            if let Some(route) = route_for(id) {
                // The shell outlives its rail, so the update fails only
                // while the window is closing.
                let _ = shell.update(cx, |shell, cx| shell.navigate(route, cx));
            }
        })
        .on_pin_toggle(move |id, _, cx| {
            if let Some(route) = route_for(id) {
                // Fails only while the window is closing.
                let _ = pin_shell.update(cx, |shell, cx| shell.toggle_pinned(route, cx));
            }
        })
        .on_menu(move |open, cx| {
            // Fails only while the window is closing.
            let _ = menu_shell.update(cx, |shell, cx| {
                if *open {
                    shell.rail_menu_opened();
                } else {
                    shell.rail_menu_closed(cx);
                }
            });
        })
        .expansion(self.rail_expansion());
        match self.prefs.mode {
            RailMode::Hover => rail.on_hover(cx.listener(|shell, hovered: &bool, _, cx| {
                if *hovered {
                    shell.rail_entered(cx);
                } else {
                    shell.rail_left(cx);
                }
            })),
            RailMode::ClickPin => {
                let shell = cx.entity().downgrade();
                rail.on_toggle(move |_, cx| {
                    // Fails only while the window is closing.
                    let _ = shell.update(cx, |shell, cx| {
                        if shell.rail_open {
                            shell.close_rail(cx);
                        } else {
                            shell.open_rail(cx);
                        }
                    });
                })
            }
        }
    }

    fn render_route(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let route = self.router.current();
        let content = div()
            .child(placeholder(
                title(*route),
                rows(*route),
                colors.muted_foreground,
            ))
            .children((*route == Route::Home).then(|| poster_grid(colors.muted, colors.border)))
            .children((*route == Route::Settings).then(|| {
                div().px_8().child(
                    Switch::new("setting-click-pin")
                        .label("Pin the sidebar on click instead of opening it on hover")
                        .checked(self.prefs.mode == RailMode::ClickPin)
                        .on_click(cx.listener(|shell, checked: &bool, _, cx| {
                            let mode = if *checked {
                                RailMode::ClickPin
                            } else {
                                RailMode::Hover
                            };
                            shell.set_rail_mode(mode, cx);
                        })),
                )
            }));
        div()
            .id("route-view")
            .flex_1()
            .min_w_0()
            .h_full()
            .overflow_y_scroll()
            .track_scroll(self.scroll.handle())
            .child(content)
            .test_support()
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let rail = self.render_rail(cx);
        let body = div().flex_1().min_h_0().relative().flex();
        // Pushed, the rail is in the layout. Otherwise its slot stays
        // collapsed-width and the rail is drawn after the route view, so an
        // expanded rail covers the content without moving it.
        let body = if self.prefs.mode == RailMode::ClickPin {
            body.child(rail).child(self.render_route(cx))
        } else {
            body.child(div().flex_none().w(px(RAIL_WIDTH)).h_full())
                .child(self.render_route(cx))
                .child(div().absolute().top_0().left_0().h_full().child(rail))
        };
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
            .child(body)
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

/// The route whose rail item has the element id `id`.
fn route_for(id: &str) -> Option<Route> {
    Route::ALL.into_iter().find(|&route| rail_id(route) == id)
}

/// The element id of `route`'s rail item.
fn rail_id(route: Route) -> String {
    format!("rail-{}", route.key())
}

pub(crate) fn title(route: Route) -> SharedString {
    match route {
        Route::Home => "Home",
        Route::Discover => "Discover",
        Route::Schedule => "Schedule",
        Route::Manga => "Manga",
        Route::Extensions => "Extensions",
        Route::Sync => "Offline / Sync",
        Route::Settings => "Settings",
    }
    .into()
}

/// Discover gets a long list so scroll restore can be tried by hand.
fn rows(route: Route) -> usize {
    match route {
        Route::Discover => 100,
        Route::Home
        | Route::Schedule
        | Route::Manga
        | Route::Extensions
        | Route::Sync
        | Route::Settings => 0,
    }
}

pub(crate) fn icon(route: Route) -> IconName {
    match route {
        Route::Home => IconName::House,
        Route::Discover => IconName::Compass,
        Route::Schedule => IconName::Calendar,
        Route::Manga => IconName::BookOpen,
        Route::Extensions => IconName::Puzzle,
        Route::Sync => IconName::FolderSync,
        Route::Settings => IconName::Settings,
    }
}

#[cfg(test)]
mod tests {
    use std::{cell::RefCell, rc::Rc, time::Duration};

    use gpui_kit::{
        AppContext, Bounds, Entity, MouseButton, NavigationDirection, Pixels, Point, ScrollDelta,
        Task, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
        base::Root,
        component::WindowExt,
        point, px, size,
        test::{ClickOptions, TestEventExt, TestWindowExt},
    };
    use ui_state::{Route, actions::OpenCommandPalette};

    use super::{AppShell, HOVER_OPEN_DELAY, RailMode, RailPrefs};
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

    const COLLAPSED: Pixels = px(64.);
    const EXPANDED: Pixels = px(260.);

    struct Shell {
        window: WindowHandle<Root>,
        view: Entity<AppShell>,
        /// Every preference set the shell asked to save, oldest first.
        saved: Rc<RefCell<Vec<RailPrefs>>>,
    }

    impl Shell {
        fn open(cx: &mut TestAppContext) -> Self {
            Self::open_with(cx, RailPrefs::default())
        }

        fn open_with(cx: &mut TestAppContext, prefs: RailPrefs) -> Self {
            let saved = Rc::new(RefCell::new(Vec::new()));
            let log = Rc::clone(&saved);
            let (window, view) = cx.update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                ui_state::actions::bind_keys(cx);
                // Animations end on their first frame, so sizes can be read
                // without waiting on the real clock.
                cx.set_reduce_motion(true);
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(800.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    cx.new(|cx| {
                        AppShell::new(
                            prefs,
                            move |prefs: &RailPrefs, _: &mut gpui_kit::App| {
                                log.borrow_mut().push(prefs.clone());
                                Task::ready(())
                            },
                            window,
                            cx,
                        )
                    })
                })
                .unwrap()
            });
            Self {
                window: window.downcast::<Root>().unwrap(),
                view,
                saved,
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

        fn hover(&self, cx: &mut TestAppContext, id: &'static str) {
            self.act(cx, |window, cx| window.hover(id, cx));
        }

        /// Moves the pointer onto the route view, off the rail.
        fn leave_rail(&self, cx: &mut TestAppContext) {
            self.act(cx, |window, cx| {
                window.pointer_move(point(px(900.), px(400.)), None, cx);
            });
        }

        /// Lets `duration` of test time pass and draws the result.
        fn wait(&self, cx: &mut TestAppContext, duration: Duration) {
            cx.executor().advance_clock(duration);
            cx.run_until_parked();
            self.act(cx, |_, _| {});
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

        fn bounds(&self, cx: &mut TestAppContext, id: &'static str) -> Bounds<Pixels> {
            let mut bounds = None;
            self.act(cx, |window, _| bounds = Some(window.find(id).bounds()));
            bounds.unwrap()
        }

        /// Whether an element with `id` was drawn in the last frame. It
        /// draws no new frame, so it sees only what the app itself drew.
        fn shows(&self, cx: &mut TestAppContext, id: &'static str) -> bool {
            cx.update_window(self.window.into(), |_, window, _| {
                window.try_find(id).is_some()
            })
            .unwrap()
        }

        fn right_click(&self, cx: &mut TestAppContext, id: &'static str) {
            self.act(cx, |window, cx| window.right_click(id, cx));
        }

        /// Clicks entry `index` (separators count) of the open popup menu.
        fn pick(&self, cx: &mut TestAppContext, index: usize) {
            self.act(cx, |window, cx| {
                window.within("popup-menu").click(index, cx);
            });
        }

        /// Resizes the window, then draws only the frames the app asks for,
        /// as the platform's frame loop would.
        fn resize(&self, cx: &mut TestAppContext, height: f32) {
            cx.simulate_window_resize(self.window.into(), size(px(1280.), px(height)));
            let settled = cx
                .update_window(self.window.into(), |_, window, cx| {
                    for _ in 0..10 {
                        if window.simulate_next_frame(cx) == 0 {
                            return true;
                        }
                        window.render_frame(cx);
                    }
                    false
                })
                .unwrap();
            assert!(settled, "the rail stops asking for frames");
        }

        fn last_saved(&self) -> Option<RailPrefs> {
            self.saved.borrow().last().cloned()
        }

        fn rail_width(&self, cx: &mut TestAppContext) -> Pixels {
            self.bounds(cx, "rail").size.width
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

    fn pinned() -> RailPrefs {
        RailPrefs::new(RailMode::ClickPin, Vec::new())
    }

    #[gpui_kit::test]
    fn hovering_the_rail_expands_it_over_the_content_after_the_delay(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let poster = shell.bounds(cx, "poster-1");
        assert_eq!(shell.rail_width(cx), COLLAPSED, "starts collapsed");

        shell.hover(cx, "rail");
        shell.wait(
            cx,
            HOVER_OPEN_DELAY.saturating_sub(Duration::from_millis(1)),
        );
        assert_eq!(shell.rail_width(cx), COLLAPSED, "not before the delay");

        shell.wait(cx, Duration::from_millis(1));
        assert_eq!(shell.rail_width(cx), EXPANDED, "open after the delay");
        assert_eq!(
            shell.bounds(cx, "poster-1"),
            poster,
            "the grid behind the rail does not move"
        );
    }

    #[gpui_kit::test]
    fn leaving_the_rail_collapses_it_at_once(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);
        assert_eq!(shell.rail_width(cx), EXPANDED, "open");

        shell.leave_rail(cx);
        assert_eq!(shell.rail_width(cx), COLLAPSED, "closed with no delay");
    }

    #[gpui_kit::test]
    fn a_pointer_passing_over_the_rail_does_not_open_it(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.hover(cx, "rail");
        shell.wait(cx, Duration::from_millis(50));
        shell.leave_rail(cx);
        shell.wait(cx, HOVER_OPEN_DELAY);

        assert_eq!(
            shell.rail_width(cx),
            COLLAPSED,
            "the pending open was dropped"
        );
    }

    #[gpui_kit::test]
    fn the_open_overlay_takes_clicks_meant_for_its_items(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);

        shell.click(cx, "rail-discover");

        assert_eq!(
            shell.route(cx),
            Route::Discover,
            "the rail item got the click"
        );
    }

    #[gpui_kit::test]
    fn with_click_pin_the_toggle_opens_the_rail_and_pushes_the_content(cx: &mut TestAppContext) {
        let shell = Shell::open_with(cx, pinned());
        let poster = shell.bounds(cx, "poster-1");

        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);
        assert_eq!(shell.rail_width(cx), COLLAPSED, "hover does nothing");

        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), EXPANDED, "the toggle opens it");
        assert_eq!(
            shell.bounds(cx, "poster-1").origin.x - poster.origin.x,
            EXPANDED - COLLAPSED,
            "the content moves over by the extra width"
        );

        shell.leave_rail(cx);
        assert_eq!(shell.rail_width(cx), EXPANDED, "it stays open off the rail");
        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), COLLAPSED, "the toggle closes it");
        assert_eq!(shell.bounds(cx, "poster-1"), poster, "the content is back");
    }

    #[gpui_kit::test]
    fn the_click_pin_switch_changes_the_mode_and_saves_it(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-settings");
        assert!(
            shell.saved.borrow().is_empty(),
            "nothing saved until a change"
        );

        shell.click(cx, "setting-click-pin");
        assert_eq!(*shell.saved.borrow(), [pinned()], "click-pin on is saved");
        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), EXPANDED, "now in click-pin mode");

        shell.click(cx, "setting-click-pin");
        assert_eq!(
            shell.saved.borrow().last(),
            Some(&RailPrefs::default()),
            "click-pin off is saved"
        );
        assert_eq!(
            shell.rail_width(cx),
            COLLAPSED,
            "leaving the mode collapses"
        );
    }

    #[gpui_kit::test]
    fn the_system_group_stays_at_the_bottom(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let rail = shell.bounds(cx, "rail");
        let settings = shell.bounds(cx, "rail-settings");

        assert!(
            rail.bottom() - settings.bottom() < px(100.),
            "settings sits near the rail's bottom edge, got {settings:?} in {rail:?}"
        );
    }

    #[gpui_kit::test]
    fn a_short_window_folds_the_bottom_items_into_more(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        assert!(shell.shows(cx, "rail-manga"), "everything fits at 800px");
        assert!(
            !shell.shows(cx, "rail-more"),
            "no More while everything fits"
        );

        shell.resize(cx, 400.);
        assert!(shell.shows(cx, "rail-home"), "the top item stays");
        assert!(!shell.shows(cx, "rail-manga"), "the bottom item folds");
        assert!(shell.shows(cx, "rail-more"), "More holds it");
        assert!(
            shell.shows(cx, "rail-settings"),
            "the system group never folds"
        );

        shell.resize(cx, 800.);
        assert!(shell.shows(cx, "rail-manga"), "growing brings it back");
        assert!(!shell.shows(cx, "rail-more"), "and More goes away");
    }

    #[gpui_kit::test]
    fn unpinning_moves_an_item_into_more_and_pinning_brings_it_back(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);

        shell.right_click(cx, "rail-discover");
        shell.pick(cx, 0);
        assert!(!shell.shows(cx, "rail-discover"), "unpinned off the rail");
        assert!(shell.shows(cx, "rail-more"), "into More");
        assert_eq!(
            shell.last_saved(),
            Some(RailPrefs::new(RailMode::Hover, vec![Route::Discover])),
            "the unpinned set is saved"
        );

        // More lists Discover, a separator, then "Pin Discover".
        shell.click(cx, "rail-more");
        shell.pick(cx, 0);
        assert_eq!(shell.route(cx), Route::Discover, "More navigates");
        // More's highlight is Button's presentation-only `selected`, which UI
        // queries can't see; it is checked by eye in the running app.

        shell.click(cx, "rail-more");
        shell.pick(cx, 2);
        assert!(shell.shows(cx, "rail-discover"), "pinned back");
        assert!(!shell.shows(cx, "rail-more"), "More is empty again");
        assert_eq!(
            shell.last_saved(),
            Some(RailPrefs::default()),
            "the pin is saved"
        );
    }

    #[gpui_kit::test]
    fn the_hover_rail_stays_open_under_its_menus(cx: &mut TestAppContext) {
        let shell = Shell::open_with(cx, RailPrefs::new(RailMode::Hover, vec![Route::Discover]));
        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);

        shell.click(cx, "rail-more");
        shell.hover(cx, "popup-menu");
        assert_eq!(shell.rail_width(cx), EXPANDED, "open under More's menu");

        // Dismissing the menu with the pointer off the rail closes the rail.
        shell.click(cx, "route-view");
        assert!(!shell.shows(cx, "popup-menu"), "the menu is gone");
        assert_eq!(shell.rail_width(cx), COLLAPSED, "and so is the rail");

        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);
        shell.right_click(cx, "rail-home");
        shell.hover(cx, "popup-menu");
        assert_eq!(shell.rail_width(cx), EXPANDED, "open under the Unpin menu");

        shell.click(cx, "route-view");
        assert_eq!(
            shell.rail_width(cx),
            COLLAPSED,
            "closed after the Unpin menu"
        );
    }

    #[gpui_kit::test]
    fn system_items_cannot_be_unpinned(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.right_click(cx, "rail-settings");
        assert!(!shell.shows(cx, "popup-menu"), "no menu on system items");
    }

    #[gpui_kit::test]
    fn a_saved_unpinned_item_starts_in_more(cx: &mut TestAppContext) {
        let shell = Shell::open_with(cx, RailPrefs::new(RailMode::Hover, vec![Route::Schedule]));
        assert!(!shell.shows(cx, "rail-schedule"));
        assert!(shell.shows(cx, "rail-more"));
    }

    #[gpui_kit::test]
    fn badges_show_collapsed_and_expanded(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        assert!(shell.shows(cx, "rail-schedule-badge"), "a pill on the icon");
        assert!(!shell.shows(cx, "rail-home-badge"), "no count, no pill");

        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);
        assert!(
            shell.shows(cx, "rail-schedule-badge"),
            "a trailing pill when expanded"
        );

        shell.leave_rail(cx);
        shell.right_click(cx, "rail-schedule");
        shell.pick(cx, 0);
        assert!(
            shell.shows(cx, "rail-more-badge"),
            "an unpinned item's count moves onto More"
        );
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

    // Must match the `any(target_os = "windows", target_os = "linux")` arm in
    // ui-state's actions.rs.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    const ALT_BACK_KEY: &str = "alt-left";
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    const ALT_FORWARD_KEY: &str = "alt-right";

    /// Alt+left/right walk the same history as the main keys on Windows and
    /// Linux.
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    #[gpui_kit::test]
    fn alt_arrows_walk_the_history(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-discover");
        shell.click(cx, "rail-schedule");
        shell.click(cx, "rail-home");

        shell.press(cx, ALT_BACK_KEY);
        assert_eq!(shell.route(cx), Route::Schedule, "alt+left goes back");

        shell.press(cx, ALT_FORWARD_KEY);
        assert_eq!(shell.route(cx), Route::Home, "alt+right goes forward");
    }
}
