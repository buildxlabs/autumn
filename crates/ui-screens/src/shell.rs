//! The window's root view: titlebar, rail and route view. See ARCHITECTURE §5.

use std::{rc::Rc, time::Duration};

use db::Db;
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, NavigationDirection, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt, Window,
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, TitleBar, WindowExt,
        button::{Button, ButtonVariants},
        carousel::CarouselState,
        input::InputState,
        label::Label,
    },
    div, px,
};
use metadata::{AniList, CatalogCache};
use ui_components::{
    ACCOUNT_ID, CarouselWheel, LruImageCache, MediaCardView, MediaCarousel, POSTER_IMAGE_CAPACITY,
    PosterGrid, RAIL_WIDTH, Rail, RailExpansion, RailItem,
    fetch_status::{
        FetchError, error_card, fetch_loader, offline_banner, offline_empty, rate_limit_chip,
    },
    hero::{HeroBannerData, hero_banner},
};
use ui_state::{
    RailBadges, Route, RouterState, ScrollRestore, SettingsTab,
    account::{AccountCommand, AccountStore},
    actions::{GoBack, GoForward, OpenCommandPalette, OpenSearch, WORKSPACE},
    fetch_status::{FetchCommand, FetchStatus, HOME_POSTERS, SectionStatus},
};

#[cfg(debug_assertions)]
use ui_state::{
    actions::{DebugAniListUnavailable, DebugRateLimited, DebugSlowFetch},
    fetch_status::DebugFetch,
};

pub use settings::PosterCacheAction;
use settings::{PosterCacheRequest, StorageStatus};

mod settings;

use crate::{
    lists::{ListsEvent, ListsScreen},
    palette::{CommandPalette, Mode, PaletteEvent},
    placeholder::{mock_media_views, placeholder},
};

/// Height of the transparent titlebar; macOS traffic lights are centred in it.
const TITLEBAR_HEIGHT: f32 = 40.;

/// A known-good AniList image standing in for a `bannerImage` until Discover
/// loads real catalog data. It is loaded through the same net-backed cache as
/// a poster.
const MOCK_BANNER: &str =
    "https://s4.anilist.co/file/anilistcdn/media/anime/banner/154587-ivXNJ23SM1xB.jpg";

/// Width of the command palette dialog.
const PALETTE_WIDTH: f32 = 560.;

/// How long the pointer rests on the rail before it expands (decision #19).
pub(crate) const HOVER_OPEN_DELAY: Duration = Duration::from_millis(150);

/// Rail entries from the top, top to bottom.
const RAIL_ROUTES: [Route; 4] = [Route::Home, Route::Discover, Route::Schedule, Route::Manga];
/// The system group at the rail's bottom, top to bottom.
/// The rail's system group, which never folds into "More".
pub const SYSTEM_ROUTES: [Route; 3] = [
    Route::Extensions,
    Route::Sync,
    Route::Settings {
        tab: SettingsTab::Appearance,
    },
];

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

/// Sends an account command to the owner of the token.
type AccountRequest = Box<dyn Fn(AccountCommand, &mut App) -> Task<()>>;

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
    fetch_status: Entity<FetchStatus>,
    account: Entity<AccountStore>,
    token_input: Entity<InputState>,
    login_url: SharedString,
    _account_events: Subscription,
    prefs: RailPrefs,
    save_prefs: SavePrefs,
    account_request: AccountRequest,
    account_busy: bool,
    account_task: Option<Task<()>>,
    poster_cache: PosterCacheRequest,
    storage_status: StorageStatus,
    storage_task: Option<Task<()>>,
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
    /// The virtualized mock media grid on Home.
    home_grid: Entity<PosterGrid>,
    /// My Lists is built only when startup supplied a catalog cache.
    lists: Option<Entity<ListsScreen>>,
    lists_events: Option<Subscription>,
    /// Bounded decoded-image cache shared by the banner and carousel.
    banner_cache: Entity<LruImageCache>,
    /// The mock carousel's paging state, owned here so it survives redraws.
    media_carousel: Entity<CarouselState>,
    /// Shared Rc so redraws clone a pointer, not the mock cards.
    carousel_items: Rc<[MediaCardView]>,
    /// Accumulates Alt+wheel delta between page turns.
    carousel_wheel: CarouselWheel,
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
    /// `save_prefs` persists preference changes and `poster_cache` handles
    /// Storage requests.
    #[expect(
        clippy::too_many_arguments,
        reason = "the shell receives its engine-side collaborators once, at construction"
    )]
    pub fn new(
        prefs: RailPrefs,
        fetch_status: Entity<FetchStatus>,
        account: Entity<AccountStore>,
        login_url: impl Into<SharedString>,
        save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
        poster_cache: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
        account_request: impl Fn(AccountCommand, &mut App) -> Task<()> + 'static,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let palette = cx.new(|cx| CommandPalette::new(window, cx));
        let palette_events = cx.subscribe_in(
            &palette,
            window,
            |shell, _, event: &PaletteEvent, window, cx| match event {
                PaletteEvent::Navigate(route) => {
                    window.close_dialog(cx);
                    shell.navigate(route.clone(), cx);
                }
            },
        );
        let badges = cx.new(|_| RailBadges::placeholder());
        let banner_cache = LruImageCache::new(POSTER_IMAGE_CAPACITY, cx);
        cx.observe(&badges, |_, _, cx| cx.notify()).detach();
        cx.observe(&fetch_status, |_, _, cx| cx.notify()).detach();
        cx.observe(&account, |_, _, cx| cx.notify()).detach();
        let account_events = cx.subscribe_in(
            &account,
            window,
            |shell, _, command: &AccountCommand, _, cx| {
                shell.request_account(command.clone(), cx);
            },
        );
        let token_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste the token from AniList"));
        let home_grid = PosterGrid::new(mock_media_views(), cx);
        let carousel_items: Rc<[MediaCardView]> = mock_media_views().into_iter().take(12).collect();
        let media_carousel = cx.new(|_| CarouselState::new(carousel_items.len()));
        let carousel_wheel = CarouselWheel::default();
        Self {
            router: RouterState::new(Route::Home),
            scroll: ScrollRestore::new(),
            focus,
            palette,
            _palette_events: palette_events,
            fetch_status,
            account,
            token_input,
            login_url: login_url.into(),
            _account_events: account_events,
            prefs,
            save_prefs: Rc::new(save_prefs),
            account_request: Box::new(account_request),
            account_busy: false,
            account_task: None,
            poster_cache: Box::new(poster_cache),
            storage_status: StorageStatus::Measuring,
            storage_task: None,
            saving: None,
            rail_open: false,
            hover_open: None,
            rail_hovered: false,
            rail_menus: 0,
            badges,
            home_grid,
            lists: None,
            lists_events: None,
            banner_cache,
            media_carousel,
            carousel_items,
            carousel_wheel,
        }
    }

    /// A shell whose command palette can search the Local platform's list
    /// entries and the AniList catalog. Startup supplies both sources.
    #[expect(
        clippy::too_many_arguments,
        reason = "composition-root constructor: shell state plus the two palette sources"
    )]
    pub fn new_with_palette_sources(
        prefs: RailPrefs,
        fetch_status: Entity<FetchStatus>,
        account: Entity<AccountStore>,
        login_url: impl Into<SharedString>,
        save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
        poster_cache: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
        account_request: impl Fn(AccountCommand, &mut App) -> Task<()> + 'static,
        db: Db,
        anilist: AniList,
        catalog: CatalogCache,
        gateway: ui_state::PlatformGateway,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let mut shell = Self::new(
            prefs,
            fetch_status,
            account,
            login_url,
            save_prefs,
            poster_cache,
            account_request,
            window,
            cx,
        );
        shell.palette.update(cx, |palette, _| {
            palette.attach_sources(db.clone(), anilist, gateway.clone());
        });
        let lists = ListsScreen::new(gateway, db, catalog, window, cx);
        let events = cx.subscribe(&lists, |shell, _, event: &ListsEvent, cx| {
            let ListsEvent::SelectTab(status) = event;
            shell.navigate(
                Route::Lists {
                    status: status.clone(),
                },
                cx,
            );
        });
        cx.observe(&lists, |_, _, cx| cx.notify()).detach();
        shell.lists = Some(lists);
        shell.lists_events = Some(events);
        shell
    }

    /// The route on screen.
    pub fn route(&self) -> Route {
        self.router.current().clone()
    }

    fn navigate(&mut self, route: Route, cx: &mut Context<'_, Self>) {
        let restore = self.router.push(route, self.scroll.offset());
        self.show(restore, cx);
    }

    /// Runs one account command. A newer command cancels the older one; the
    /// engine publishes the resulting account event.
    fn request_account(&mut self, command: AccountCommand, cx: &mut Context<'_, Self>) {
        // Replacing the task drops and cancels an older command in flight.
        self.account_busy = true;
        let task = (self.account_request)(command, cx);
        self.account_task = Some(cx.spawn(async move |shell, cx| {
            task.await;
            let _ = shell.update(cx, |shell, cx| {
                shell.account_busy = false;
                shell.account_task = None;
                cx.notify();
            });
        }));
        cx.notify();
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
            if matches!(
                self.route(),
                Route::Settings {
                    tab: SettingsTab::Storage
                }
            ) {
                self.request_poster_cache(PosterCacheAction::Measure, cx);
            }
            if let (Route::Lists { status }, Some(lists)) = (self.route(), &self.lists) {
                lists.update(cx, |lists, cx| lists.set_tab(status, cx));
            }
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
        if let Some(at) = self.prefs.unpinned.iter().position(|r| r == &route) {
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
                    .min_w_0()
                    .flex()
                    .justify_center()
                    .child(search_pill(cx)),
            )
            .children(self.fetch_status.read(cx).countdown().map(rate_limit_chip))
    }

    fn render_rail(&self, cx: &mut Context<'_, Self>) -> Rail {
        let current = self.router.current().clone();
        let badges = self.badges.read(cx);
        let item = |route: &Route| RailItem {
            id: rail_id(route).into(),
            label: title(route),
            icon: icon(route),
            active: route == &current
                || matches!(
                    (route, &current),
                    (Route::Settings { .. }, Route::Settings { .. })
                ),
            badge: badges.count(route),
            pinned: !self.prefs.unpinned.contains(route),
        };
        let items = RAIL_ROUTES.iter().map(&item).collect();
        let system = SYSTEM_ROUTES.iter().map(&item).collect();
        let shell = cx.entity().downgrade();
        let pin_shell = shell.clone();
        let menu_shell = shell.clone();
        let account_name = self.account.read(cx).name().unwrap_or("Guest").to_owned();
        let rail = Rail::new(items, system, account_name, move |id, _, cx| {
            if id.as_ref() == ACCOUNT_ID {
                let _ = shell.update(cx, |shell, cx| {
                    shell.navigate(
                        Route::Settings {
                            tab: SettingsTab::Account,
                        },
                        cx,
                    );
                });
                return;
            }
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
        let route = self.route();
        let route_view = div()
            .id("route-view")
            .flex_1()
            .min_w_0()
            .h_full()
            .test_support();
        match &route {
            Route::Settings { tab } => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(self.render_settings(*tab, cx))
                .into_any_element(),
            Route::Home => route_view
                .flex()
                .flex_col()
                .child(div().flex_none().child(placeholder(
                    title(&route),
                    rows(&route),
                    colors.muted_foreground,
                )))
                .child(div().flex_1().min_h_0().child(self.render_home_posters(cx)))
                .into_any_element(),
            Route::Lists { .. } => match &self.lists {
                Some(lists) => route_view.child(lists.clone()).into_any_element(),
                None => route_view
                    .overflow_y_scroll()
                    .track_scroll(self.scroll.handle())
                    .child(placeholder(
                        title(&route),
                        rows(&route),
                        colors.muted_foreground,
                    ))
                    .into_any_element(),
            },
            route => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .children((route == &Route::Discover).then(|| self.render_discover_heroes(cx)))
                .child(placeholder(
                    title(route),
                    rows(route),
                    colors.muted_foreground,
                ))
                .into_any_element(),
        }
    }

    #[cfg(debug_assertions)]
    fn debug_fetch(&self, fetch: DebugFetch, cx: &mut Context<'_, Self>) {
        self.fetch_status.update(cx, |_, cx| {
            FetchStatus::request(FetchCommand::Debug(fetch), cx);
        });
    }

    fn render_home_posters(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let state = self.fetch_status.read(cx).section(HOME_POSTERS);
        let retry = cx.listener(|shell, _: &ClickEvent, _, cx| {
            shell.fetch_status.update(cx, |_, cx| {
                FetchStatus::request(FetchCommand::Retry(HOME_POSTERS), cx);
            });
        });
        match state {
            Some(SectionStatus::Failed(detail)) => div().px_8().child(error_card(
                "fetch-error",
                FetchError::Request(detail),
                retry,
            )),
            Some(SectionStatus::AniListUnavailable) => div().px_8().child(error_card(
                "fetch-error",
                FetchError::AniListUnavailable,
                retry,
            )),
            Some(SectionStatus::OfflineEmpty) => {
                div().px_8().child(offline_empty("offline-empty", retry))
            }
            Some(SectionStatus::OfflineCached(age)) => div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().px_8().pb_4().child(offline_banner(age.as_str())))
                .child(div().flex_1().min_h_0().child(self.home_grid.clone())),
            _ => div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().flex_1().min_h_0().child(self.home_grid.clone())),
        }
    }

    /// Mock heroes on the placeholder route: one with a `bannerImage`, one
    /// without, so both states can be checked by hand until #62 replaces it.
    fn render_discover_heroes(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mock = |banner: Option<String>| HeroBannerData {
            title: "Frieren: Beyond Journey's End".into(),
            synopsis: "After the hero party defeats the Demon King, the elf mage Frieren \
                       sets out on a new journey to understand the people she once travelled \
                       with &amp; the <i>time</i> they shared. Along the way she meets a \
                       young mage &amp; learns what it means to remember."
                .into(),
            banner,
        };
        let actions = || {
            div()
                .flex()
                .gap_2()
                .child(Button::new("hero-watch").label("Watch now"))
                .child(Button::new("hero-details").label("Details"))
        };
        div()
            .px_8()
            .pb_6()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("With a banner image"),
            )
            .child(hero_banner(
                "hero-with",
                mock(Some(MOCK_BANNER.to_owned())),
                &self.banner_cache,
                colors.accent,
                colors.background,
                actions(),
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("Without a banner image"),
            )
            .child(hero_banner(
                "hero-without",
                mock(None),
                &self.banner_cache,
                colors.accent,
                colors.background,
                actions(),
            ))
            .child(
                div()
                    .text_sm()
                    .text_color(colors.muted_foreground)
                    .child("Media carousel — drag, use the arrows, or Alt+wheel"),
            )
            .child(MediaCarousel::new(
                "discover-carousel",
                self.media_carousel.clone(),
                Rc::clone(&self.carousel_items),
                self.banner_cache.clone(),
                self.carousel_wheel.clone(),
            ))
    }
}

impl Render for AppShell {
    fn render(&mut self, _: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let expiring = self.account.read(cx).expiring();
        let expired = self.account.read(cx).expired();
        let prompt = if expired {
            Some(AccountPrompt::Expired)
        } else {
            expiring.then_some(AccountPrompt::Expiring)
        };
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
        let shell = div()
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
            .children(self.fetch_status.read(cx).loading().then(fetch_loader))
            .children(prompt.map(|prompt| account_banner(cx, prompt)))
            .child(body);
        #[cfg(debug_assertions)]
        let shell = shell
            .on_action(cx.listener(|shell, _: &DebugSlowFetch, _, cx| {
                shell.debug_fetch(DebugFetch::Slow, cx);
            }))
            .on_action(cx.listener(|shell, _: &DebugRateLimited, _, cx| {
                shell.debug_fetch(DebugFetch::RateLimited, cx);
            }))
            .on_action(cx.listener(|shell, _: &DebugAniListUnavailable, _, cx| {
                shell.debug_fetch(DebugFetch::Unavailable, cx);
            }));
        shell
    }
}

/// The search pill; a click opens the command palette.
fn search_pill(cx: &mut Context<'_, AppShell>) -> impl IntoElement {
    let theme = cx.theme();
    div()
        .id("search-pill")
        .w_full()
        .max_w(px(320.))
        .min_w_0()
        .overflow_hidden()
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

/// Urgency of the account's re-login prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AccountPrompt {
    /// The token still works, but is close to expiry.
    Expiring,
    /// The token has expired and AniList is read-only.
    Expired,
}

/// A shell-wide re-login prompt while the token is expiring or expired.
fn account_banner(cx: &mut Context<'_, AppShell>, prompt: AccountPrompt) -> impl IntoElement {
    let shell = cx.entity().downgrade();
    let colors = cx.theme().colors;
    div()
        .id("account-banner")
        .test_support()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_8()
        .py_2()
        .bg(colors.accent)
        .text_color(colors.accent_foreground)
        .child(Label::new(match prompt {
            AccountPrompt::Expired => "AniList is read-only until you log in again",
            AccountPrompt::Expiring => "AniList token expires soon — log in again to keep syncing",
        }))
        .child(
            Button::new("account-banner-open")
                .label("Log in again")
                .on_click(move |_, _, cx| {
                    let _ = shell.update(cx, |shell, cx| {
                        shell.navigate(
                            Route::Settings {
                                tab: SettingsTab::Account,
                            },
                            cx,
                        );
                    });
                }),
        )
}

/// The route whose rail item has the element id `id`.
fn route_for(id: &str) -> Option<Route> {
    Route::ALL
        .iter()
        .find(|route| rail_id(route) == id)
        .cloned()
}

/// The element id of `route`'s rail item.
fn rail_id(route: &Route) -> String {
    format!("rail-{}", route.key())
}

pub(crate) fn title(route: &Route) -> SharedString {
    match route {
        Route::Home => "Home",
        Route::Discover => "Discover",
        Route::Lists { .. } => "My Lists",
        Route::Schedule => "Schedule",
        Route::Manga => "Manga",
        Route::Extensions => "Extensions",
        Route::Sync => "Offline / Sync",
        Route::Settings { .. } => "Settings",
    }
    .into()
}

/// Discover gets a long list so scroll restore can be tried by hand.
fn rows(route: &Route) -> usize {
    match route {
        Route::Discover => 100,
        Route::Home
        | Route::Lists { .. }
        | Route::Schedule
        | Route::Manga
        | Route::Extensions
        | Route::Sync
        | Route::Settings { .. } => 0,
    }
}

pub(crate) fn icon(route: &Route) -> IconName {
    match route {
        Route::Home => IconName::House,
        Route::Discover => IconName::Compass,
        Route::Lists { .. } => IconName::List,
        Route::Schedule => IconName::Calendar,
        Route::Manga => IconName::BookOpen,
        Route::Extensions => IconName::Puzzle,
        Route::Sync => IconName::FolderSync,
        Route::Settings { .. } => IconName::Settings,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        time::Duration,
    };

    use domain::{AniListAccountState, AniListWait, AppEvent, SectionFetch};
    use gpui_kit::{
        AppContext, Bounds, Entity, MouseButton, NavigationDirection, Pixels, Point, ScrollDelta,
        Task, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
        base::Root,
        component::WindowExt,
        point, px, size,
        test::{ClickOptions, TestEventExt, TestWindowExt},
    };
    use ui_state::{
        Route, SettingsTab,
        account::AccountStore,
        actions::OpenCommandPalette,
        events::{Sender, channel, start_pump},
        fetch_status::{FetchCommand, FetchStatus, HOME_POSTERS},
    };

    use super::{AppShell, HOVER_OPEN_DELAY, PosterCacheAction, RailMode, RailPrefs};
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
            Self::open_state(cx, prefs, None)
        }

        fn open_with_account(cx: &mut TestAppContext, session: AniListAccountState) -> Self {
            Self::open_state(cx, RailPrefs::default(), Some(session))
        }

        fn open_state(
            cx: &mut TestAppContext,
            prefs: RailPrefs,
            session: Option<AniListAccountState>,
        ) -> Self {
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
                    let fetch_status = cx.new(|_| FetchStatus::default());
                    let account = cx.new(|_| {
                        session.map_or_else(AccountStore::default, AccountStore::from_state)
                    });
                    cx.new(|cx| {
                        AppShell::new(
                            prefs,
                            fetch_status,
                            account,
                            "https://example.test/login".to_owned(),
                            move |prefs: &RailPrefs, _: &mut gpui_kit::App| {
                                log.borrow_mut().push(prefs.clone());
                                Task::ready(())
                            },
                            {
                                let bytes = Cell::new(42);
                                move |action, _| {
                                    if action == PosterCacheAction::Clear {
                                        bytes.set(0);
                                    }
                                    Task::ready(Ok(bytes.get()))
                                }
                            },
                            |_, _| Task::ready(()),
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

        fn events(&self, cx: &mut TestAppContext) -> Sender {
            let (sender, receiver) = channel();
            cx.update(|cx| {
                let store = self.view.read(cx).fetch_status.clone();
                start_pump(cx, receiver, move |event, cx| {
                    store.update(cx, |store, cx| store.apply(event, cx));
                })
                .detach();
            });
            sender
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

    #[gpui_kit::test]
    fn the_event_pump_shows_only_requests_that_reach_300_ms(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let sender = shell.events(cx);
        let now = cx.executor().now();

        sender.send(AppEvent::FetchActivity(Some(now))).unwrap();
        cx.run_until_parked();
        shell.wait(cx, Duration::from_millis(299));
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("fetch-loader").is_none(),
                "loader must be hidden"
            );
        });
        shell.wait(cx, Duration::from_millis(1));
        shell.act(cx, |window, _| {
            assert!(
                window.find("fetch-loader").visible(),
                "loader appears at 300ms"
            );
            assert_eq!(
                window.find("fetch-progress").bounds().size.height,
                px(2.),
                "top bar is 2px tall"
            );
        });

        sender.send(AppEvent::FetchActivity(None)).unwrap();
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("fetch-loader").is_none(),
                "loader must be hidden"
            );
        });

        sender
            .send(AppEvent::FetchActivity(Some(cx.executor().now())))
            .unwrap();
        cx.run_until_parked();
        shell.wait(cx, Duration::from_millis(100));
        sender.send(AppEvent::FetchActivity(None)).unwrap();
        cx.run_until_parked();
        shell.wait(cx, Duration::from_millis(500));
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("fetch-loader").is_none(),
                "loader must be hidden"
            );
        });
    }

    #[gpui_kit::test]
    fn the_titlebar_counts_down_until_anilist_can_accept_requests(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        cx.simulate_window_resize(shell.window.into(), size(px(420.), px(800.)));
        let sender = shell.events(cx);
        sender
            .send(AppEvent::AniListRateLimit(Some(AniListWait::lockout(
                cx.executor().now() + Duration::from_millis(2500),
            ))))
            .unwrap();
        cx.run_until_parked();

        for seconds in [3, 2, 1] {
            shell.act(cx, |window, _| {
                let chip = window.find("anilist-rate-limit");
                assert!(chip.visible(), "active limit is visible");
                assert!(
                    chip.bounds().right() <= px(420.),
                    "the chip fits a narrow window"
                );
                assert!(
                    window.find("search-pill").bounds().right() <= chip.bounds().left(),
                    "search leaves room for the chip"
                );
                assert!(
                    has_text(window, &format!("AniList rate limit · {seconds}s")),
                    "expected {seconds}s remaining"
                );
            });
            shell.wait(
                cx,
                Duration::from_millis(if seconds == 3 { 500 } else { 1000 }),
            );
        }
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("anilist-rate-limit").is_none(),
                "chip clears at zero"
            );
        });
    }

    #[gpui_kit::test]
    fn the_titlebar_keeps_the_chip_until_the_queue_clears(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let sender = shell.events(cx);
        let until = cx.executor().now() + Duration::from_secs(1);
        sender
            .send(AppEvent::AniListRateLimit(Some(AniListWait::queued(until))))
            .unwrap();
        cx.run_until_parked();

        shell.wait(cx, Duration::from_secs(1));
        shell.act(cx, |window, _| {
            assert!(
                has_text(window, "AniList rate limit · 0s"),
                "the queue still has a waiter"
            );
        });

        sender.send(AppEvent::AniListRateLimit(None)).unwrap();
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("anilist-rate-limit").is_none(),
                "the last waiter left"
            );
        });
    }

    #[gpui_kit::test]
    fn section_fetch_outcomes_keep_other_content_and_retry_the_section(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let sender = shell.events(cx);
        let retries = Rc::new(RefCell::new(Vec::new()));
        let _retry = cx.update(|cx| {
            let store = shell.view.read(cx).fetch_status.clone();
            let retries = Rc::clone(&retries);
            cx.subscribe(&store, move |_, event: &FetchCommand, _| {
                retries.borrow_mut().push(*event);
            })
        });
        let show = |cx: &mut TestAppContext, state| {
            sender
                .send(AppEvent::SectionFetch {
                    section: HOME_POSTERS,
                    state,
                })
                .unwrap();
            cx.run_until_parked();
        };

        show(cx, SectionFetch::Failed("The connection timed out.".into()));
        shell.act(cx, |window, _| {
            assert!(
                window.find("fetch-error").visible(),
                "only the failed section gets a card"
            );
            assert!(
                has_text(window, "Couldn’t load this section"),
                "generic error title"
            );
            assert!(
                has_text(window, "The connection timed out."),
                "failure detail"
            );
            assert!(
                window.find("rail-home").visible(),
                "the shell still renders"
            );
        });
        shell.click(cx, "fetch-retry");

        show(cx, SectionFetch::AniListUnavailable);
        shell.act(cx, |window, _| {
            assert!(
                has_text(window, "AniList unavailable"),
                "403 has a dedicated message"
            );
        });
        shell.click(cx, "fetch-retry");

        show(
            cx,
            SectionFetch::Offline {
                last_updated: Some(std::time::SystemTime::now()),
            },
        );
        shell.act(cx, |window, _| {
            assert!(
                window.find("offline-banner").visible(),
                "the banner accompanies cached content"
            );
            assert!(
                has_text(window, "Offline: last updated just now"),
                "the banner includes the cache age"
            );
            assert_grid_shown(window, "cached content remains visible");
        });

        show(cx, SectionFetch::Offline { last_updated: None });
        shell.act(cx, |window, _| {
            assert!(
                window.find("offline-empty").visible(),
                "offline without cache has an empty state"
            );
            assert!(
                window.try_find("poster-grid-row-0").is_none(),
                "there is no cached content"
            );
        });
        shell.click(cx, "fetch-retry");
        assert_eq!(
            *retries.borrow(),
            [FetchCommand::Retry(HOME_POSTERS); 3],
            "Retry requests target the failed section"
        );

        show(cx, SectionFetch::Ready);
        shell.act(cx, |window, _| {
            assert_grid_shown(window, "successful fetch restores the section");
            assert!(
                window.try_find("offline-empty").is_none(),
                "recovery clears the empty state"
            );
        });
    }

    /// The grid is a full-height flex child, not a zero-height auto div.
    fn assert_grid_shown(window: &gpui_kit::Window, message: &str) {
        assert!(window.find("poster-grid").visible(), "{message}");
        assert!(
            window.find("poster-grid").bounds().size.height > px(0.),
            "the grid has room to lay out rows"
        );
        assert!(
            window.try_find("poster-grid-row-0").is_some(),
            "the first row is built"
        );
    }

    fn has_text(window: &gpui_kit::Window, expected: &str) -> bool {
        gpui_kit::base::test_support::texts(window)
            .iter()
            .any(|text| text.text() == expected)
    }

    fn pinned() -> RailPrefs {
        RailPrefs::new(RailMode::ClickPin, Vec::new())
    }

    #[gpui_kit::test]
    fn hovering_the_rail_expands_it_over_the_content_after_the_delay(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let poster = shell.bounds(cx, "poster-grid-row-0");
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
            shell.bounds(cx, "poster-grid-row-0"),
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
        let poster = shell.bounds(cx, "poster-grid-row-0");

        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);
        assert_eq!(shell.rail_width(cx), COLLAPSED, "hover does nothing");

        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), EXPANDED, "the toggle opens it");
        assert_eq!(
            shell.bounds(cx, "poster-grid-row-0").origin.x - poster.origin.x,
            EXPANDED - COLLAPSED,
            "the content moves over by the extra width"
        );

        shell.leave_rail(cx);
        assert_eq!(shell.rail_width(cx), EXPANDED, "it stays open off the rail");
        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), COLLAPSED, "the toggle closes it");
        assert_eq!(
            shell.bounds(cx, "poster-grid-row-0"),
            poster,
            "the content is back"
        );
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
    fn storage_tab_shows_the_poster_cache_and_clears_it(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-storage");
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                gpui_kit::base::test_support::texts(window)
                    .iter()
                    .any(|text| text.visible() && text.text() == "42 bytes"),
                "Storage should display the current poster cache size"
            );
        });

        shell.click(cx, "clear-poster-cache");
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                gpui_kit::base::test_support::texts(window)
                    .iter()
                    .any(|text| text.visible() && text.text() == "0 bytes"),
                "Storage should display the size after clear"
            );
        });

        shell.click(cx, "back");
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("setting-click-pin").is_some(),
                "the settings tab is part of navigation history"
            );
        });
    }

    #[gpui_kit::test]
    fn clicking_the_account_row_opens_the_account_tab(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);

        shell.click(cx, "rail-account");

        assert_eq!(
            shell.route(cx),
            Route::Settings {
                tab: SettingsTab::Account
            }
        );
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("settings-account").is_some(),
                "the Account tab is selected"
            );
        });
    }

    #[gpui_kit::test]
    fn account_tab_shows_the_name_and_relogin_banner(cx: &mut TestAppContext) {
        let shell = Shell::open_with_account(
            cx,
            AniListAccountState::Active {
                name: "FrierenFan".to_owned(),
                avatar: None,
                expires_at: std::time::UNIX_EPOCH + std::time::Duration::from_hours(500_000),
                expiring: true,
            },
        );

        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-account");
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            let texts = gpui_kit::base::test_support::texts(window);
            assert!(
                texts
                    .iter()
                    .any(|text| text.visible() && text.text() == "FrierenFan"),
                "the AniList name is shown"
            );
            assert!(
                texts
                    .iter()
                    .any(|text| text.visible() && text.text().contains("expires soon")),
                "the re-login prompt is shown"
            );
            assert!(
                !texts
                    .iter()
                    .any(|text| text.visible() && text.text().contains("read-only")),
                "a still-valid token is not described as read-only"
            );
        });
    }

    #[gpui_kit::test]
    fn the_account_tab_renders_the_avatar(cx: &mut TestAppContext) {
        let shell = Shell::open_with_account(
            cx,
            AniListAccountState::Active {
                name: "FrierenFan".to_owned(),
                avatar: Some("https://example.test/avatar.gif".to_owned()),
                expires_at: std::time::UNIX_EPOCH + std::time::Duration::from_hours(500_000),
                expiring: false,
            },
        );

        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-account");
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("account-avatar").is_some(),
                "the animated avatar URL is rendered"
            );
        });
    }

    #[gpui_kit::test]
    fn an_expired_token_shows_a_shell_wide_relogin_banner(cx: &mut TestAppContext) {
        let shell = Shell::open_with_account(
            cx,
            AniListAccountState::Expired {
                name: None,
                avatar: None,
            },
        );

        shell.act(cx, |window, _| {
            assert!(
                window.try_find("account-banner-open").is_some(),
                "the re-login banner is visible outside Settings"
            );
        });
        shell.click(cx, "account-banner-open");
        assert_eq!(
            shell.route(cx),
            Route::Settings {
                tab: SettingsTab::Account
            }
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

    #[gpui_kit::test]
    fn the_discover_placeholder_shows_a_hero_with_and_without_a_banner(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        assert!(!shell.shows(cx, "hero-with-banner"), "Home has no hero");

        shell.click(cx, "rail-discover");

        assert!(
            shell.shows(cx, "hero-with-banner"),
            "the banner render exists"
        );
        assert!(
            shell.shows(cx, "hero-with-image"),
            "a media with a bannerImage loads it through the image cache"
        );
        assert!(
            shell.shows(cx, "hero-without-banner"),
            "the fallback render exists too"
        );
        assert!(
            !shell.shows(cx, "hero-without-image"),
            "a media without a bannerImage gets the gradient, not an img"
        );
    }

    #[gpui_kit::test]
    fn the_mock_carousel_renders_cards_and_its_arrow_pages(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-discover");

        assert!(
            shell.shows(cx, "discover-carousel"),
            "the carousel is on screen"
        );
        assert!(
            shell.shows(cx, "discover-carousel-card-0"),
            "the first media card is drawn"
        );
        let selected = |cx: &mut TestAppContext| {
            shell.view.read_with(cx, |shell, cx| {
                shell
                    .media_carousel
                    .read_with(cx, |state, _| state.selected_index())
            })
        };
        assert_eq!(selected(cx), Some(0));

        // The placeholder's long list puts the arrows below the fold.
        shell.act(cx, |window, cx| {
            window.scroll(
                "route-view",
                ScrollDelta::Pixels(point(px(0.), px(-520.))),
                cx,
            );
        });
        shell.click(cx, "discover-carousel-next");
        assert_eq!(selected(cx), Some(1), "the next arrow paged one card");
        shell.click(cx, "discover-carousel-previous");
        assert_eq!(selected(cx), Some(0), "the previous arrow paged back");
    }
}
