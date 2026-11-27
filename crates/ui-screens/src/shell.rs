//! The window's root view: titlebar, rail and route view. See ARCHITECTURE §5.

use std::{path::PathBuf, rc::Rc, time::Duration};

use db::Db;
use domain::TransferDirection;
use gpui_kit::{
    App, AppContext, ClickEvent, Context, Entity, FocusHandle, InteractiveElement, IntoElement,
    MouseButton, NavigationDirection, ParentElement, Pixels, Point, Render, Resource, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Task, TestSupportExt, WeakEntity, Window,
    assets::IconName,
    component::{
        ActiveTheme, Disableable, Icon, Sizable, TitleBar, WindowExt,
        button::{Button, ButtonVariants},
        input::InputState,
        label::Label,
        notification::Notification,
        slider::{SliderEvent, SliderState, SliderValue},
    },
    div, px,
};
use metadata::{AniList, CatalogCache};
use ui_components::{
    ACCOUNT_ID, AutoNextView, ChapterMark, LruImageCache, PLAYBACK_HUD_HEIGHT,
    POSTER_IMAGE_CAPACITY, PlaybackHud, PlaybackHudActions, PlaybackHudView, PosterGrid,
    RAIL_WIDTH, Rail, RailExpansion, RailItem, SkipKind,
    fetch_status::{
        FetchError, error_card, fetch_loader, offline_banner, offline_empty, rate_limit_chip,
    },
};
use ui_state::{
    EntryLibraryStore, EntryTab, LibraryStore, RailBadges, Route, RouterState, ScanOutcome,
    ScanSummariesStore, ScrollRestore, SearchFilters, SettingsTab,
    account::{AccountCommand, AccountStore},
    actions::{
        GoBack, GoForward, OpenCommandPalette, OpenLibraryExplorer, OpenSearch, PLAYER, PlayPause,
        ScanLibrary, SeekBack, SeekForward, SkipChapter, StopPlayback, WORKSPACE,
    },
    entry_store::EntryStore,
    fetch_status::{FetchCommand, FetchStatus, HOME_POSTERS, SectionStatus},
    home_store::HomeStore,
    playback_store::PlaybackStore,
};

#[cfg(debug_assertions)]
use ui_state::{
    actions::{DebugAniListUnavailable, DebugRateLimited, DebugSlowFetch},
    fetch_status::DebugFetch,
};

use ::settings::PlayerChoice;
pub use ::settings::{GeneralSettings, LibrarySettings, PlaybackSettings, TorrentSettings};
pub use settings::{
    GeneralServices, LibraryServices, PlaybackServices, PlaybackTestError, PlayerOption,
    PosterCacheAction, TorrentCacheAction, TorrentServices,
};
use settings::{PlaybackTestStatus, PosterCacheRequest, StorageStatus, TorrentCacheStatus};

mod onboarding;
mod settings;

use crate::{
    discover::{DiscoverLoader, DiscoverView},
    entry::{EntryEditor, EntryScreen, observe as observe_entry, render as render_entry},
    home::{HomeCarousels, observe as observe_home, render as render_home},
    library_explorer::LibraryExplorerScreen,
    lists::{ListsEvent, ListsScreen},
    palette::{CommandPalette, Mode, PaletteEvent},
    placeholder::{episode_gallery, mock_media_views, placeholder},
    scan_summaries::{MediaSearchDialog, MediaSearchEvent, ScanSummariesScreen},
    search::{SearchEvent, SearchScreen},
};

/// Height of the transparent titlebar; macOS traffic lights are centred in it.
const TITLEBAR_HEIGHT: f32 = 40.;

/// Width of the command palette dialog.
const PALETTE_WIDTH: f32 = 560.;

/// How long the pointer rests on the rail before it expands (decision #19).
pub(crate) const HOVER_OPEN_DELAY: Duration = Duration::from_millis(150);

/// Rail entries from the top, top to bottom.
const RAIL_ROUTES: [Route; 3] = [Route::Home, Route::Discover, Route::Schedule];
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

/// Moves the confirmed files to the trash.
pub(crate) type TrashAction = Rc<dyn Fn(&[PathBuf], &mut App)>;

/// Sends an account command to the owner of the token.
type AccountRequest = Box<dyn Fn(AccountCommand, &mut App) -> Task<()>>;

/// The shell around every route: titlebar on top, rail on the left.
///
/// It owns the navigation history and the route view's scroll handle, so
/// leaving a route saves its offset and coming back restores it. It also
/// hosts the command palette.
#[expect(
    clippy::struct_excessive_bools,
    reason = "the wizard's keychain availability and flags are independent shell inputs"
)]
pub struct AppShell {
    router: RouterState<Route>,
    /// Async screens emit `ContentLoaded`; subscribe the shell to call
    /// `scroll.content_loaded()` so their saved offset survives the load.
    scroll: ScrollRestore,
    focus: FocusHandle,
    shell_handle: WeakEntity<Self>,
    palette: Entity<CommandPalette>,
    _palette_events: Subscription,
    fetch_status: Entity<FetchStatus>,
    /// The Anime Entry screen's store; `show` opens the current Entry.
    entry_store: Entity<EntryStore>,
    /// The Entry list editor's UI state (the notes input).
    entry_editor: Entity<EntryEditor>,
    /// Keeps the editor sync subscribed to Entry store notifications.
    _entry_events: Subscription,
    /// The Entry's Library tab: local files as episode cards (#132).
    entry_library: Entity<EntryLibraryStore>,
    /// The AniList search behind the Library tab's Re-match (#133), built
    /// once startup has an AniList client.
    entry_rematch: Option<Entity<MediaSearchDialog>>,
    /// The Home screen's store; `show` opens Home's sections.
    home_store: Entity<HomeStore>,
    /// The one playback session, mirrored from the engine for the HUD.
    playback_store: Entity<PlaybackStore>,
    /// The HUD's focus handle, so Player key bindings apply only to it.
    hud_focus: FocusHandle,
    /// Paging state for Home's carousels.
    home_carousels: Entity<HomeCarousels>,
    /// Keeps the carousel counts subscribed to Home store notifications.
    _home_events: Subscription,
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
    /// The Playback tab's current values and its composition-root callbacks.
    playback: PlaybackSettings,
    playback_services: PlaybackServices,
    /// The General tab's flag and save callback (#138).
    general: GeneralSettings,
    general_services: GeneralServices,
    /// The wizard's current step: navigation only, never a settings copy.
    onboarding_step: usize,
    /// Whether AniList login can store a token in the OS keychain (ADR 0002).
    keychain_available: bool,
    players: Vec<PlayerOption>,
    players_task: Option<Task<()>>,
    player_pick_task: Option<Task<()>>,
    player_pick_error: Option<String>,
    playback_test_status: PlaybackTestStatus,
    playback_test_task: Option<Task<()>>,
    playback_error: Option<String>,
    save_playback_task: Option<Task<()>>,
    /// The Library tab's values, store and composition-root callback (#127).
    library: LibrarySettings,
    library_services: LibraryServices,
    library_store: Entity<LibraryStore>,
    _library_events: Subscription,
    threshold_slider: Entity<SliderState>,
    _threshold_events: Subscription,
    library_error: Option<String>,
    library_pick_task: Option<Task<()>>,
    save_library_task: Option<Task<()>>,
    /// The Storage tab's torrent values and composition-root callbacks (#229).
    torrent: TorrentSettings,
    torrent_services: TorrentServices,
    torrent_error: Option<String>,
    torrent_pick_task: Option<Task<()>>,
    save_torrent_task: Option<Task<()>>,
    torrent_cache_status: TorrentCacheStatus,
    torrent_cache_task: Option<Task<()>>,
    /// The scan log screen, built once startup has an AniList client (#134).
    scan_summaries: Entity<ScanSummariesStore>,
    scan_summaries_screen: Option<Entity<ScanSummariesScreen>>,
    /// The Library Explorer sheet, built once startup has an AniList client.
    explorer: Entity<ui_state::LibraryExplorerStore>,
    explorer_screen: Option<Entity<LibraryExplorerScreen>>,
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
    /// The virtualized mock media grid behind the debug fetch-status demo:
    /// built, and shown on Home, once a debug fetch reports on it.
    home_grid: Option<Entity<PosterGrid>>,
    /// My Lists is built only when startup supplied a catalog cache.
    lists: Option<Entity<ListsScreen>>,
    lists_events: Option<Subscription>,
    /// The Discover screen, loaded stale-first when that route is shown.
    discover_view: Entity<DiscoverView>,
    /// Shared decoded-image cache for the Entry banner and carousels.
    banner_cache: Entity<LruImageCache>,
    search: Option<Entity<SearchScreen>>,
    search_events: Option<Subscription>,
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
        clippy::fn_params_excessive_bools,
        reason = "the shell receives its engine-side collaborators once"
    )]
    pub fn new(
        prefs: RailPrefs,
        playback: PlaybackSettings,
        playback_services: PlaybackServices,
        library: LibrarySettings,
        library_services: LibraryServices,
        library_store: Entity<LibraryStore>,
        scan_summaries: Entity<ScanSummariesStore>,
        explorer: Entity<ui_state::LibraryExplorerStore>,
        fetch_status: Entity<FetchStatus>,
        account: Entity<AccountStore>,
        login_url: impl Into<SharedString>,
        save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
        poster_cache: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
        account_request: impl Fn(AccountCommand, &mut App) -> Task<()> + 'static,
        discover_loader: DiscoverLoader,
        entry_store: Entity<EntryStore>,
        entry_library: Entity<EntryLibraryStore>,
        home_store: Entity<HomeStore>,
        playback_store: Entity<PlaybackStore>,
        general: GeneralSettings,
        general_services: GeneralServices,
        keychain_available: bool,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        Self::new_with_torrent(
            prefs,
            playback,
            playback_services,
            library,
            library_services,
            TorrentSettings::default(),
            TorrentServices::inert(),
            library_store,
            scan_summaries,
            explorer,
            fetch_status,
            account,
            login_url,
            save_prefs,
            poster_cache,
            account_request,
            discover_loader,
            entry_store,
            entry_library,
            home_store,
            playback_store,
            general,
            general_services,
            keychain_available,
            window,
            cx,
        )
    }

    /// The full constructor, including the torrent controls on Storage.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::fn_params_excessive_bools,
        reason = "the shell receives its engine-side collaborators and builds every store and subscription at construction"
    )]
    pub fn new_with_torrent(
        prefs: RailPrefs,
        playback: PlaybackSettings,
        playback_services: PlaybackServices,
        library: LibrarySettings,
        library_services: LibraryServices,
        torrent: TorrentSettings,
        torrent_services: TorrentServices,
        library_store: Entity<LibraryStore>,
        scan_summaries: Entity<ScanSummariesStore>,
        explorer: Entity<ui_state::LibraryExplorerStore>,
        fetch_status: Entity<FetchStatus>,
        account: Entity<AccountStore>,
        login_url: impl Into<SharedString>,
        save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
        poster_cache: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
        account_request: impl Fn(AccountCommand, &mut App) -> Task<()> + 'static,
        discover_loader: DiscoverLoader,
        entry_store: Entity<EntryStore>,
        entry_library: Entity<EntryLibraryStore>,
        home_store: Entity<HomeStore>,
        playback_store: Entity<PlaybackStore>,
        general: GeneralSettings,
        general_services: GeneralServices,
        keychain_available: bool,
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
                PaletteEvent::ScanLibrary => {
                    window.close_dialog(cx);
                    shell.start_scan(cx);
                }
                PaletteEvent::OpenExplorer => {
                    window.close_dialog(cx);
                    shell.open_explorer(&OpenLibraryExplorer, window, cx);
                }
            },
        );
        let badges = cx.new(|_| RailBadges::placeholder());
        let banner_cache = LruImageCache::new(POSTER_IMAGE_CAPACITY, cx);
        cx.observe(&badges, |_, _, cx| cx.notify()).detach();
        cx.observe(&fetch_status, |shell, status, cx| {
            if shell.home_grid.is_none() && status.read(cx).section(HOME_POSTERS).is_some() {
                shell.home_grid = Some(PosterGrid::new(mock_media_views(), cx));
            }
            cx.notify();
        })
        .detach();
        cx.observe(&account, |shell, _, cx| {
            // A platform switch is published as an account change. Refresh the
            // open list so its entries come from the new active platform.
            if let (Route::Lists { status }, Some(lists)) = (shell.route(), shell.lists.as_ref()) {
                lists.update(cx, |lists, cx| lists.set_tab(status, cx));
            }
            cx.notify();
        })
        .detach();
        let account_events = cx.subscribe_in(
            &account,
            window,
            |shell, _, command: &AccountCommand, _, cx| {
                shell.request_account(command.clone(), cx);
            },
        );
        let threshold_slider = cx.new(|_| {
            SliderState::new()
                .min(0.0)
                .max(100.0)
                .step(5.0)
                .default_value(f32::from(library.matching_threshold))
        });
        let threshold_events = cx.subscribe(&threshold_slider, |shell, _, event, cx| {
            if let SliderEvent::Release(SliderValue::Single(value)) = event {
                shell.library_threshold_changed(*value, cx);
            }
        });
        let library_events = cx.observe_in(&library_store, window, |shell, store, window, cx| {
            if let Some(outcome) = store.update(cx, |store, _| store.take_finish()) {
                shell.show_scan_toast(outcome, window, cx);
            }
            if let Some(error) = store.update(cx, |store, _| store.take_scan_error()) {
                window.push_notification(Notification::error(error), cx);
            }
            cx.notify();
        });
        let token_input =
            cx.new(|cx| InputState::new(window, cx).placeholder("Paste the token from AniList"));
        let entry_editor = EntryEditor::new(window, cx);
        let entry_events = observe_entry(&entry_store, &entry_editor, window, cx);
        let home_carousels = HomeCarousels::new(cx);
        let home_events = observe_home(&home_store, &home_carousels, cx);
        // A list save can land after the user is back on Home: reload the
        // list sections when the Entry store changes while Home is shown.
        cx.observe(&entry_store, |shell, _, cx| {
            if shell.route() == Route::Home {
                shell.home_store.update(cx, HomeStore::reload_lists);
            }
        })
        .detach();
        cx.observe_in(&entry_library, window, |_, store, window, cx| {
            if let Some(error) = store.update(cx, |store, _| store.take_error()) {
                window.push_notification(Notification::error(error), cx);
            }
            cx.notify();
        })
        .detach();
        // The HUD is drawn from the playback store; engine events notify it
        // at most four times a second.
        cx.observe(&playback_store, |_, _, cx| cx.notify()).detach();
        // Home is the first route: load it now, as `show` does on return.
        home_store.update(cx, HomeStore::open);
        let user = account.read(cx).name().map(str::to_owned);
        let discover_view = DiscoverView::new(user, discover_loader, banner_cache.clone(), cx);
        let shell = cx.entity().downgrade();
        discover_view.update(cx, |view, _| {
            view.set_on_open(move |id, _, cx| {
                let _ = shell.update(cx, |shell, cx| {
                    shell.navigate(Route::Entry { id, tab: None }, cx);
                });
            });
        });
        // Missed Sequels reads the signed-in account's collection, so a new
        // account replaces the old data instead of showing it under a new name.
        cx.observe(&account, |shell, account, cx| {
            let user = account.read(cx).name().map(str::to_owned);
            let reload = shell.route() == Route::Discover;
            shell.discover_view.update(cx, |view, cx| {
                if view.set_user(user) && reload {
                    view.load(cx);
                }
            });
        })
        .detach();
        let mut shell = Self {
            router: RouterState::new(if general.onboarding_completed {
                Route::Home
            } else {
                Route::Onboarding
            }),
            scroll: ScrollRestore::new(),
            focus,
            shell_handle: cx.entity().downgrade(),
            palette,
            _palette_events: palette_events,
            fetch_status,
            entry_store,
            entry_editor,
            _entry_events: entry_events,
            entry_library,
            entry_rematch: None,
            home_store,
            playback_store,
            hud_focus: cx.focus_handle(),
            home_carousels,
            _home_events: home_events,
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
            playback,
            playback_services,
            general,
            general_services,
            onboarding_step: 0,
            keychain_available,
            players: Vec::new(),
            players_task: None,
            player_pick_task: None,
            player_pick_error: None,
            playback_test_status: PlaybackTestStatus::Idle,
            playback_test_task: None,
            playback_error: None,
            save_playback_task: None,
            library,
            library_services,
            library_store,
            _library_events: library_events,
            threshold_slider,
            _threshold_events: threshold_events,
            library_error: None,
            library_pick_task: None,
            save_library_task: None,
            torrent,
            torrent_services,
            torrent_error: None,
            torrent_pick_task: None,
            save_torrent_task: None,
            torrent_cache_status: TorrentCacheStatus::Measuring,
            torrent_cache_task: None,
            scan_summaries,
            scan_summaries_screen: None,
            explorer,
            explorer_screen: None,
            saving: None,
            rail_open: false,
            hover_open: None,
            rail_hovered: false,
            rail_menus: 0,
            badges,
            home_grid: None,
            lists: None,
            lists_events: None,
            discover_view,
            banner_cache,
            search: None,
            search_events: None,
        };
        // Detection is I/O, so it starts here rather than in `render()`.
        shell.request_players(cx);
        shell
    }

    /// A shell whose command palette can search the Local platform's list
    /// entries and the AniList catalog. Startup supplies both sources.
    #[expect(
        clippy::too_many_arguments,
        clippy::too_many_lines,
        clippy::fn_params_excessive_bools,
        reason = "composition-root constructor builds every store and screen once"
    )]
    pub fn new_with_palette_sources(
        prefs: RailPrefs,
        playback: PlaybackSettings,
        playback_services: PlaybackServices,
        library: LibrarySettings,
        library_services: LibraryServices,
        torrent: TorrentSettings,
        torrent_services: TorrentServices,
        library_store: Entity<LibraryStore>,
        scan_summaries: Entity<ScanSummariesStore>,
        explorer: Entity<ui_state::LibraryExplorerStore>,
        fetch_status: Entity<FetchStatus>,
        account: Entity<AccountStore>,
        login_url: impl Into<SharedString>,
        save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
        poster_cache: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
        account_request: impl Fn(AccountCommand, &mut App) -> Task<()> + 'static,
        discover_loader: DiscoverLoader,
        entry_store: Entity<EntryStore>,
        entry_library: Entity<EntryLibraryStore>,
        home_store: Entity<HomeStore>,
        playback_store: Entity<PlaybackStore>,
        general: GeneralSettings,
        general_services: GeneralServices,
        keychain_available: bool,
        db: Db,
        anilist: AniList,
        catalog: CatalogCache,
        gateway: ui_state::PlatformGateway,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Self {
        let explorer_folders = library.folders.clone();
        let mut shell = Self::new_with_torrent(
            prefs,
            playback,
            playback_services,
            library,
            library_services,
            torrent,
            torrent_services,
            library_store,
            scan_summaries,
            explorer,
            fetch_status,
            account,
            login_url,
            save_prefs,
            poster_cache,
            account_request,
            discover_loader,
            entry_store,
            entry_library,
            home_store,
            playback_store,
            general,
            general_services,
            keychain_available,
            window,
            cx,
        );
        shell.palette.update(cx, |palette, _| {
            palette.attach_sources(db.clone(), anilist.clone(), gateway.clone());
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

        let summaries_handle = cx.entity().downgrade();
        let summaries = cx.new(|cx| {
            ScanSummariesScreen::new(
                shell.scan_summaries.clone(),
                anilist.clone(),
                shell.scroll.handle().clone(),
                window,
                cx,
            )
        });
        summaries.update(cx, |screen, _| {
            screen.set_on_open(move |id, _, cx| {
                let _ = summaries_handle.update(cx, |shell, cx| {
                    shell.navigate(Route::Entry { id, tab: None }, cx);
                });
            });
        });
        shell.scan_summaries_screen = Some(summaries);
        cx.observe_in(&shell.scan_summaries, window, |_, store, window, cx| {
            if let Some(error) = store.update(cx, |store, _| store.take_error()) {
                window.push_notification(Notification::error(error), cx);
            }
        })
        .detach();

        let explorer = cx.new(|cx| {
            LibraryExplorerScreen::new(
                shell.explorer.clone(),
                shell.scan_summaries.clone(),
                anilist.clone(),
                explorer_folders,
                window,
                cx,
            )
        });
        let explorer_host = cx.entity().downgrade();
        explorer.update(cx, |screen, _| {
            screen.set_on_trash(move |paths, window, cx| {
                let _ = explorer_host.update(cx, |shell, cx| {
                    let store = shell.explorer.clone();
                    let trash = Rc::new(move |paths: &[PathBuf], cx: &mut App| {
                        store.update(cx, |store, cx| store.trash(paths, cx));
                    });
                    Self::confirm_trash(paths, trash, window, cx);
                });
            });
        });
        shell.explorer_screen = Some(explorer);
        cx.observe_in(&shell.explorer, window, |_, store, window, cx| {
            if let Some(error) = store.update(cx, |store, _| store.take_error()) {
                window.push_notification(Notification::error(error), cx);
            }
        })
        .detach();

        let rematch = cx.new(|cx| MediaSearchDialog::new(window, anilist.clone(), cx));
        // The dialog is opened on one file; the match applies to the whole
        // selection, which is what the user picked Re-match for.
        cx.subscribe(&rematch, |shell, _, event, cx| {
            let MediaSearchEvent::Matched { anilist_id, .. } = event;
            shell
                .entry_library
                .update(cx, |store, cx| store.rematch(*anilist_id, cx));
        })
        .detach();
        shell.entry_rematch = Some(rematch);

        let search = SearchScreen::new(SearchFilters::default(), anilist, window, cx);
        let shell_handle = cx.entity().downgrade();
        search.update(cx, |search, _cx| {
            search.set_on_open(move |id, _, cx| {
                let _ = shell_handle.update(cx, |shell, cx| {
                    shell.navigate(Route::Entry { id, tab: None }, cx);
                });
            });
        });
        let events = cx.subscribe(&search, |shell, _, event: &SearchEvent, cx| match event {
            SearchEvent::Filters(filters) => {
                shell.navigate(Route::Search(filters.clone()), cx);
            }
            SearchEvent::Loaded => {
                shell.scroll.content_loaded();
                if let Some(search) = &shell.search {
                    let search = search.clone();
                    // Defer: a cache hit emits this while the screen is being
                    // updated from `show`, and re-entering it would panic.
                    cx.spawn(async move |_, cx| {
                        search.update(cx, SearchScreen::content_loaded);
                    })
                    .detach();
                }
            }
        });
        cx.observe(&search, |_, _, cx| cx.notify()).detach();
        shell.search = Some(search);
        shell.search_events = Some(events);
        shell
    }

    /// The route on screen.
    pub fn route(&self) -> Route {
        self.router.current().clone()
    }

    /// Opens `route`, saving the current route's scroll offset.
    pub(crate) fn navigate(&mut self, route: Route, cx: &mut Context<'_, Self>) {
        let offset = self.leaving_scroll(cx);
        let restore = self.router.push(route, offset);
        self.show(restore, cx);
    }

    /// Opens the AniList search to re-match the Library tab's selection.
    pub(crate) fn open_entry_rematch(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) {
        let first = self.entry_library.read(cx).selected().into_iter().next();
        let (Some(dialog), Some(path)) = (self.entry_rematch.clone(), first) else {
            return;
        };
        if window.has_active_dialog(cx) {
            return;
        }
        let view = dialog.clone();
        window.open_dialog(cx, move |dialog, _, _| {
            dialog
                .w(px(480.))
                .title("Re-match selected files")
                .child(view.clone())
        });
        dialog.update(cx, |dialog, cx| dialog.open(path, window, cx));
    }

    /// Shows `tab` of the Entry on screen. A tab switch replaces the route,
    /// so Back leaves the Entry instead of stepping through its tabs.
    pub(crate) fn select_entry_tab(&mut self, tab: EntryTab, cx: &mut Context<'_, Self>) {
        if let Route::Entry { id, .. } = self.route() {
            self.router.replace(Route::Entry { id, tab: Some(tab) });
            cx.notify();
        }
    }

    /// The offset to save for the route being left. Search scrolls inside its
    /// own poster grid, not the route view.
    fn leaving_scroll(&self, cx: &mut Context<'_, Self>) -> Point<Pixels> {
        if let (Route::Search(_), Some(search)) = (&self.route(), &self.search) {
            search.read_with(cx, SearchScreen::scroll_offset)
        } else {
            self.scroll.offset()
        }
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

    fn scan_library(&mut self, _: &ScanLibrary, _: &mut Window, cx: &mut Context<'_, Self>) {
        self.start_scan(cx);
    }

    fn open_explorer(
        &mut self,
        _: &OpenLibraryExplorer,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(screen) = self.explorer_screen.clone() else {
            return;
        };
        let folders = self.library.folders.clone();
        screen.update(cx, |screen, cx| {
            screen.set_folders(folders, cx);
            screen.open(window, cx);
        });
    }

    /// The one sheet can only close after the user confirms the file count.
    /// Asks before moving `paths` to the OS trash (OD-6), naming the file
    /// count; `trash` runs only on confirm.
    pub(crate) fn confirm_trash(
        paths: Vec<PathBuf>,
        trash: TrashAction,
        window: &mut Window,
        cx: &mut App,
    ) {
        if window.has_active_dialog(cx) || paths.is_empty() {
            return;
        }
        let count = paths.len();
        window.open_dialog(cx, move |dialog, _, _| {
            let trash = Rc::clone(&trash);
            let confirm_paths = paths.clone();
            dialog.title("Move to trash").child(
                div()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(Label::new(format!(
                        "Move {count} file(s) to the trash? They are not deleted permanently."
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .child(
                                Button::new("confirm-trash")
                                    .label("Move to trash")
                                    .on_click(move |_, window, cx| {
                                        window.close_dialog(cx);
                                        trash(&confirm_paths, cx);
                                    }),
                            )
                            .child(Button::new("cancel-trash").label("Cancel").on_click(
                                |_, window, cx| {
                                    window.close_dialog(cx);
                                },
                            )),
                    ),
            )
        });
    }

    /// Shows the scan summary once, when the store hands over its outcome.
    /// The Scan Summaries link arrives with that route in #134.
    fn show_scan_toast(&self, outcome: ScanOutcome, window: &mut Window, cx: &mut App) {
        let failed = outcome.error.is_some();
        let message = if let Some(error) = outcome.error {
            format!("Library scan failed: {error}")
        } else {
            format!(
                "{} files matched to {} media, {} unmatched",
                outcome.files_matched,
                self.library_store
                    .read(cx)
                    .summary()
                    .map_or(0, |s| s.by_media.len()),
                outcome.files_unmatched
            )
        };
        let shell = self.shell_handle.clone();
        let notification = if failed {
            Notification::error(message)
        } else {
            Notification::success(message)
        };
        window.push_notification(
            notification.action(move |_, _, _cx| {
                let shell = shell.clone();
                Button::new("scan-summaries-link")
                    .label("Scan Summaries")
                    .on_click(move |_, _, cx| {
                        let _ = shell.update(cx, |shell, cx| {
                            shell.navigate(Route::ScanSummaries, cx);
                        });
                    })
            }),
            cx,
        );
    }

    fn go_back(&mut self, _: &GoBack, _: &mut Window, cx: &mut Context<'_, Self>) {
        let offset = self.leaving_scroll(cx);
        let restore = self.router.back(offset);
        self.show(restore, cx);
    }

    fn go_forward(&mut self, _: &GoForward, _: &mut Window, cx: &mut Context<'_, Self>) {
        let offset = self.leaving_scroll(cx);
        let restore = self.router.forward(offset);
        self.show(restore, cx);
    }

    /// Scrolls the new route to its saved offset, if the route changed.
    ///
    /// [`ScrollRestore`] keeps the offset so an async screen's
    /// `ContentLoaded` can apply it again after layout clamps it.
    fn show(&mut self, restore: Option<Point<Pixels>>, cx: &mut Context<'_, Self>) {
        // Back from an Entry keeps the selected summary; only a fresh visit
        // reloads the log. The router restores the scroll offset separately.
        if self.route() == Route::ScanSummaries && restore.is_none() {
            self.scan_summaries.update(cx, ScanSummariesStore::open);
        }
        match self.route() {
            Route::Entry { id, .. } => {
                self.entry_store.update(cx, |store, cx| store.open(id, cx));
                self.entry_library
                    .update(cx, |store, cx| store.open(id, cx));
            }
            Route::Home => {
                self.home_store.update(cx, HomeStore::open);
            }
            _ => {}
        }
        if let Some(offset) = restore {
            self.scroll.restore(offset);
            if self.route() == Route::Discover {
                self.discover_view.update(cx, DiscoverView::load);
            }
            if matches!(
                self.route(),
                Route::Settings {
                    tab: SettingsTab::Storage
                }
            ) {
                self.request_poster_cache(PosterCacheAction::Measure, cx);
                self.request_torrent_cache(TorrentCacheAction::Measure, cx);
            }
            if let (Route::Lists { status }, Some(lists)) = (self.route(), &self.lists) {
                lists.update(cx, |lists, cx| lists.set_tab(status, cx));
            }
            if let (Route::Search(filters), Some(search)) = (self.route(), &self.search) {
                search.update(cx, |search, cx| {
                    search.restore_scroll(offset, cx);
                    search.set_filters(filters, cx);
                });
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

    /// The bottom bar, present on every route while a session runs. It reads
    /// only the store; every command goes through it and out over the bridge.
    #[expect(
        clippy::too_many_lines,
        reason = "one method builds the view model, callbacks and key bindings for the bar"
    )]
    fn render_playback_hud(
        &self,
        window: &mut Window,
        cx: &mut Context<'_, Self>,
    ) -> Option<gpui_kit::AnyElement> {
        let (state, details, chapters, skip, auto_next, closed) = {
            let playback = self.playback_store.read(cx);
            (
                playback.state(),
                playback.details().cloned(),
                playback.chapters().to_vec(),
                playback.skip_kind(),
                playback.auto_next(),
                playback.closed(),
            )
        };
        let Some(state) = state else {
            return closed.then(|| {
                div()
                    .id("playback-hud-closed")
                    .test_support()
                    .h(px(PLAYBACK_HUD_HEIGHT))
                    .flex_none()
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_t_1()
                    .border_color(cx.theme().colors.border)
                    .bg(cx.theme().colors.sidebar)
                    .child(Label::new("Player closed"))
                    .into_any_element()
            });
        };

        let poster = details
            .as_ref()
            .and_then(|details| details.poster.as_deref())
            .map(|url| Resource::Uri(url.to_owned().into()));
        let view = PlaybackHudView {
            poster,
            media_title: details
                .as_ref()
                .map_or_else(
                    || format!("Media {}", state.media.get()),
                    |d| d.media_title.clone(),
                )
                .into(),
            episode: Some(state.episode),
            episode_title: details
                .as_ref()
                .and_then(|details| details.episode_title.clone())
                .map(Into::into),
            player: self.player_name().into(),
            position: state.position,
            duration: state.duration,
            paused: state.paused,
            chapters: chapters
                .iter()
                .map(|chapter| ChapterMark {
                    start: chapter.start,
                })
                .collect(),
            skip: skip.map(|kind| match kind {
                domain::ChapterKind::Ending => SkipKind::Ending,
                _ => SkipKind::Opening,
            }),
            auto_next: auto_next.map(|next| AutoNextView {
                episode: next.episode,
                remaining: next.remaining,
            }),
        };
        let store = self.playback_store.clone();
        let actions = PlaybackHudActions::new()
            .on_toggle_pause({
                let store = store.clone();
                move |_, cx| store.update(cx, PlaybackStore::toggle_pause)
            })
            .on_seek({
                let store = store.clone();
                move |seconds, _, cx| store.update(cx, |store, cx| store.seek_to(seconds, cx))
            })
            .on_skip_back({
                let store = store.clone();
                move |_, cx| store.update(cx, |store, cx| store.seek_by(-10., cx))
            })
            .on_skip_forward({
                let store = store.clone();
                move |_, cx| store.update(cx, |store, cx| store.seek_by(10., cx))
            })
            .on_stop({
                let store = store.clone();
                move |_, cx| store.update(cx, PlaybackStore::stop)
            })
            .on_skip({
                let store = store.clone();
                move |_, cx| store.update(cx, PlaybackStore::skip_chapter)
            })
            .on_play_next({
                let store = store.clone();
                move |_, cx| store.update(cx, PlaybackStore::play_next_now)
            })
            .on_cancel_auto_next(move |_, cx| store.update(cx, PlaybackStore::cancel_auto_next));

        Some(
            div()
                .id("playback-hud")
                .test_support()
                .track_focus(&self.hud_focus)
                .key_context(PLAYER)
                .on_action(cx.listener(|shell, _: &PlayPause, _, cx| {
                    shell.playback_store.update(cx, PlaybackStore::toggle_pause);
                }))
                .on_action(cx.listener(|shell, _: &SeekBack, _, cx| {
                    shell
                        .playback_store
                        .update(cx, |store, cx| store.seek_by(-10., cx));
                }))
                .on_action(cx.listener(|shell, _: &SeekForward, _, cx| {
                    shell
                        .playback_store
                        .update(cx, |store, cx| store.seek_by(10., cx));
                }))
                .on_action(cx.listener(|shell, _: &StopPlayback, _, cx| {
                    shell.playback_store.update(cx, PlaybackStore::stop);
                }))
                .on_action(cx.listener(|shell, _: &SkipChapter, _, cx| {
                    shell.playback_store.update(cx, PlaybackStore::skip_chapter);
                }))
                .on_click(cx.listener(|shell, _, window, cx| {
                    window.focus(&shell.hud_focus.clone(), cx);
                }))
                .child(PlaybackHud::new(
                    "playback-hud-bar",
                    view,
                    actions,
                    window,
                    cx,
                ))
                .into_any_element(),
        )
    }

    /// The player named in Settings, resolved against detected players.
    fn player_name(&self) -> String {
        match &self.playback.player {
            PlayerChoice::Auto => self
                .players
                .first()
                .map_or_else(|| "Auto".to_owned(), |player| player.label.to_string()),
            PlayerChoice::Installed { kind } | PlayerChoice::Custom { kind, .. } => self
                .players
                .iter()
                .find(|player| player.kind == *kind)
                .map_or_else(
                    || kind.label().to_owned(),
                    |player| player.label.to_string(),
                ),
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

    #[expect(
        clippy::too_many_lines,
        reason = "one renderer lists every route's view and its fallback"
    )]
    fn render_route(&self, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let route = self.route();
        let route_view = div()
            .id("route-view")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .test_support();
        match &route {
            Route::Settings { tab } => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(self.render_settings(*tab, cx))
                .into_any_element(),
            Route::Onboarding => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(onboarding::render(self, cx))
                .into_any_element(),
            Route::Home => {
                let mut home = route_view
                    .flex()
                    .flex_col()
                    .overflow_y_scroll()
                    .track_scroll(self.scroll.handle());
                // The debug fetch-status demo grid appears, above the
                // widgets, once a debug fetch has reported on it.
                if let Some(grid) = self.home_grid.clone() {
                    home = home.child(
                        div()
                            .flex_none()
                            .h(px(480.))
                            .child(self.render_home_posters(grid, cx)),
                    );
                }
                home.child(render_home(
                    &self.home_store,
                    &self.home_carousels,
                    &self.banner_cache,
                    &self.library_store,
                    &self.playback_store,
                    cx,
                ))
                .into_any_element()
            }
            Route::Search(_) => match &self.search {
                Some(search) => route_view.child(search.clone()).into_any_element(),
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
            Route::Discover => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(self.discover_view.clone())
                .into_any_element(),
            Route::Schedule => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(episode_gallery(&self.banner_cache))
                .into_any_element(),
            Route::Entry { id, tab } => route_view
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(render_entry(
                    &EntryScreen {
                        store: &self.entry_store,
                        editor: &self.entry_editor,
                        library: &self.entry_library,
                        playback: &self.playback_store,
                        cache: &self.banner_cache,
                    },
                    *id,
                    *tab,
                    cx,
                ))
                .into_any_element(),
            Route::ScanSummaries => match &self.scan_summaries_screen {
                Some(screen) => route_view.child(screen.clone()).into_any_element(),
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

    fn render_home_posters(
        &self,
        grid: Entity<PosterGrid>,
        cx: &mut Context<'_, Self>,
    ) -> impl IntoElement {
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
                .child(div().flex_1().min_h_0().child(grid)),
            _ => div()
                .flex()
                .flex_col()
                .size_full()
                .child(div().flex_1().min_h_0().child(grid)),
        }
    }
}

#[cfg(test)]
impl AppShell {
    /// The Entry store, for screen tests that exercise Entry actions.
    pub(crate) fn entry_store(&self) -> &Entity<EntryStore> {
        &self.entry_store
    }

    /// The Entry editor, for screen tests that exercise carousel actions.
    pub(crate) fn entry_editor(&self) -> &Entity<EntryEditor> {
        &self.entry_editor
    }

    /// The banner cache shared with Entry's banner and carousels.
    pub(crate) fn banner_cache(&self) -> &Entity<LruImageCache> {
        &self.banner_cache
    }
}

impl Render for AppShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let expiring = self.account.read(cx).expiring();
        let expired = self.account.read(cx).expired();
        let prompt = if expired {
            Some(AccountPrompt::Expired)
        } else {
            expiring.then_some(AccountPrompt::Expiring)
        };
        let transfer = self.account.read(cx).transfer_preview().cloned();
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
            .on_action(cx.listener(Self::scan_library))
            .on_action(cx.listener(Self::open_explorer))
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
            .children(transfer.map(|preview| transfer_banner(cx, preview.direction)))
            .child(body)
            .children(self.render_playback_hud(window, cx));
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

/// A shell-wide prompt to review a list transfer offered on connect or
/// sign-out. The preview itself lives in Settings → Account.
fn transfer_banner(
    cx: &mut Context<'_, AppShell>,
    direction: TransferDirection,
) -> impl IntoElement {
    let shell = cx.entity().downgrade();
    let colors = cx.theme().colors;
    div()
        .id("transfer-banner")
        .test_support()
        .flex()
        .items_center()
        .justify_between()
        .gap_3()
        .px_8()
        .py_2()
        .bg(colors.accent)
        .text_color(colors.accent_foreground)
        .child(Label::new(format!(
            "{} lists are ready to transfer — review the preview",
            direction.source_label()
        )))
        .child(
            Button::new("transfer-banner-open")
                .label("Review transfer")
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
        Route::Onboarding => "Setup",
        Route::Home => "Home",
        Route::Entry { .. } => "Entry",
        Route::Discover => "Discover",
        Route::Lists { .. } => "My Lists",
        Route::Search(_) => "Advanced Search",
        Route::Schedule => "Schedule",
        Route::ScanSummaries => "Scan Summaries",
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
        Route::Onboarding
        | Route::Entry { .. }
        | Route::Home
        | Route::Lists { .. }
        | Route::Search(_)
        | Route::Schedule
        | Route::ScanSummaries
        | Route::Extensions
        | Route::Sync
        | Route::Settings { .. } => 0,
    }
}

pub(crate) fn icon(route: &Route) -> IconName {
    match route {
        Route::Onboarding | Route::Home => IconName::House,
        Route::Entry { .. } => IconName::Clapperboard,
        Route::Discover => IconName::Compass,
        Route::Lists { .. } => IconName::List,
        Route::Search(_) => IconName::Search,
        Route::Schedule => IconName::Calendar,
        Route::ScanSummaries | Route::Sync => IconName::FolderSync,
        Route::Extensions => IconName::Puzzle,
        Route::Settings { .. } => IconName::Settings,
    }
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "test fixtures are indexed by fixed JSON keys"
    )]

    use std::{
        cell::{Cell, RefCell},
        path::PathBuf,
        rc::Rc,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
        time::Duration,
    };

    use crate::discover::{
        DiscoverSection, DiscoverStore, DiscoverUpdate, Section, SectionEntries, test_media,
    };
    use axum::{Json, Router, extract::State, http::StatusCode, routing::post};
    use domain::{
        AniListAccountState, AniListWait, AppEvent, MediaId, PlaybackState, SectionFetch,
    };
    use gpui_kit::{
        App, AppContext, Bounds, Entity, MouseButton, NavigationDirection, Pixels, Point,
        ScrollDelta, Task, TestAppContext, WindowBounds, WindowHandle, WindowOptions,
        base::Root,
        component::WindowExt,
        point, px, size,
        test::{ClickOptions, TestEventExt, TestWindowExt},
    };
    use serde_json::{Value, json};
    use tokio::net::TcpListener;
    use ui_state::entry_store::EntryStore;
    use ui_state::home_store::HomeStore;
    use ui_state::test_support::{PlaybackCall, PlaybackServices, pending_services};
    use ui_state::{
        PlatformGateway, PlaybackStore, Route, SearchFilters, SettingsTab,
        account::{AccountCommand, AccountStore},
        actions::OpenCommandPalette,
        bridge::BridgeError,
        events::{Sender, channel, start_pump},
        fetch_status::{FetchCommand, FetchStatus, HOME_POSTERS},
    };

    use super::{
        AppShell, HOVER_OPEN_DELAY, PosterCacheAction, RailMode, RailPrefs, SearchScreen,
        TorrentCacheAction, TorrentServices, TorrentSettings,
    };
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
        /// Every account the Discover loader was given, oldest first.
        users: Rc<RefCell<Vec<Option<String>>>>,
        /// Every account command emitted by the shell, oldest first.
        commands: Rc<RefCell<Vec<AccountCommand>>>,
        /// Every torrent settings save requested by the shell, oldest first.
        torrent_saved: Rc<RefCell<Vec<TorrentSettings>>>,
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

        /// A shell whose playback store records commands through a fake engine.
        fn open_with_playback(cx: &mut TestAppContext, services: PlaybackServices) -> Self {
            Self::open_playback_state(cx, RailPrefs::default(), None, services)
        }

        fn open_state(
            cx: &mut TestAppContext,
            prefs: RailPrefs,
            session: Option<AniListAccountState>,
        ) -> Self {
            Self::open_playback_state(cx, prefs, session, PlaybackServices::default())
        }

        #[expect(
            clippy::too_many_lines,
            reason = "the test shell builds every store and fake once"
        )]
        fn open_playback_state(
            cx: &mut TestAppContext,
            prefs: RailPrefs,
            session: Option<AniListAccountState>,
            playback_services: PlaybackServices,
        ) -> Self {
            let saved = Rc::new(RefCell::new(Vec::new()));
            let log = Rc::clone(&saved);
            let users = Rc::new(RefCell::new(Vec::new()));
            let seen = Rc::clone(&users);
            let commands = Rc::new(RefCell::new(Vec::new()));
            let sent = Rc::clone(&commands);
            let torrent_saved = Rc::new(RefCell::new(Vec::new()));
            let recorded = Rc::clone(&torrent_saved);
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
                    let entry_store = cx.new(|_| EntryStore::new(pending_services()));
                    let home_store = cx.new(|_| HomeStore::new(pending_services()));
                    cx.new(|cx| {
                        AppShell::new_with_torrent(
                            prefs,
                            settings::PlaybackSettings::default(),
                            crate::shell::PlaybackServices::inert(),
                            settings::LibrarySettings::default(),
                            crate::shell::LibraryServices::inert(),
                            TorrentSettings::default(),
                            TorrentServices::new(
                                move |settings, _| {
                                    recorded.borrow_mut().push(settings.clone());
                                    Task::ready(())
                                },
                                {
                                    let bytes = Cell::new(84);
                                    move |action, _| {
                                        if action == TorrentCacheAction::Clear {
                                            bytes.set(0);
                                        }
                                        Task::ready(Ok(bytes.get()))
                                    }
                                },
                                PathBuf::from("/cache/torrents"),
                            ),
                            cx.new(|_| ui_state::LibraryStore::new(pending_services())),
                            cx.new(|_| ui_state::ScanSummariesStore::new(pending_services())),
                            cx.new(|_| ui_state::LibraryExplorerStore::new(pending_services())),
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
                            move |command, _| {
                                sent.borrow_mut().push(command);
                                Task::ready(())
                            },
                            Rc::new(move |user: Option<String>, _: &App| {
                                seen.borrow_mut().push(user);
                                let store = DiscoverStore::from_sections(vec![Section::new(
                                    DiscoverSection::Popular,
                                    SectionEntries::Media(vec![test_media("A shell card")]),
                                    SectionFetch::Ready,
                                )]);
                                Task::ready(Ok::<_, BridgeError>(DiscoverUpdate::new(store)))
                            }),
                            entry_store,
                            cx.new(|_| ui_state::EntryLibraryStore::new(pending_services())),
                            home_store,
                            cx.new(|_| PlaybackStore::new(std::sync::Arc::new(playback_services))),
                            settings::GeneralSettings {
                                onboarding_completed: true,
                            },
                            crate::shell::GeneralServices::inert(),
                            true,
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
                users,
                commands,
                torrent_saved,
            }
        }

        fn events(&self, cx: &mut TestAppContext) -> Sender {
            let (sender, receiver) = channel();
            cx.update(|cx| {
                let store = self.view.read(cx).fetch_status.clone();
                let playback = self.view.read(cx).playback_store.clone();
                start_pump(cx, receiver, move |event, cx| {
                    store.update(cx, |store, cx| store.apply(event.clone(), cx));
                    playback.update(cx, |store, cx| store.apply(event, cx));
                })
                .detach();
            });
            sender
        }

        fn commands(&self) -> Vec<AccountCommand> {
            self.commands.borrow().clone()
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

        fn click_at(&self, cx: &mut TestAppContext, id: &'static str, offset: Point<Pixels>) {
            self.act(cx, |window, cx| {
                window.click_with_options(id, ClickOptions::new().with_offset(offset), cx);
            });
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
        shell.act(cx, |window, cx| {
            window.within("fetch-error").click("fetch-retry", cx);
        });

        show(cx, SectionFetch::AniListUnavailable);
        shell.act(cx, |window, _| {
            assert!(
                has_text(window, "AniList unavailable"),
                "403 has a dedicated message"
            );
        });
        shell.act(cx, |window, cx| {
            window.within("fetch-error").click("fetch-retry", cx);
        });

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
        shell.act(cx, |window, cx| {
            window.within("offline-empty").click("fetch-retry", cx);
        });
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

    #[gpui_kit::test]
    fn the_playback_hud_tracks_the_session_and_hides_when_it_ends(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .expect("a test runtime");
        cx.update(|cx| ui_state::bridge::init(cx, runtime.handle().clone()));

        let services = PlaybackServices::default();
        let calls = services.clone();
        let shell = Shell::open_with_playback(cx, services);
        let sender = shell.events(cx);

        sender
            .send(AppEvent::PlaybackState(PlaybackState {
                media: MediaId::new(7),
                episode: 3,
                position: Duration::from_secs(42),
                duration: Some(Duration::from_mins(24)),
                paused: false,
            }))
            .unwrap();
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("playback-hud").is_some(),
                "the HUD is shown"
            );
            assert!(has_text(window, "0:42 / 24:00"), "the position is rendered");
        });

        let width = shell.bounds(cx, "playback-hud-seek-track").size.width;
        shell.click_at(cx, "playback-hud-seek-track", point(width / 2., px(2.)));
        cx.run_until_parked();
        assert_eq!(
            calls.calls(),
            [PlaybackCall::Seek(720.)],
            "a click on the bar seeks to that position"
        );

        sender
            .send(AppEvent::PlaybackStopped { unexpected: false })
            .unwrap();
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                window.try_find("playback-hud").is_none(),
                "the HUD is hidden"
            );
        });
    }

    fn pinned() -> RailPrefs {
        RailPrefs::new(RailMode::ClickPin, Vec::new())
    }

    #[gpui_kit::test]
    fn hovering_the_rail_expands_it_over_the_content_after_the_delay(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        let content = shell.bounds(cx, "home-view").origin;
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
            shell.bounds(cx, "home-view").origin,
            content,
            "the content behind the rail does not move"
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
        let content = shell.bounds(cx, "home-view").origin;

        shell.hover(cx, "rail");
        shell.wait(cx, HOVER_OPEN_DELAY);
        assert_eq!(shell.rail_width(cx), COLLAPSED, "hover does nothing");

        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), EXPANDED, "the toggle opens it");
        assert_eq!(
            shell.bounds(cx, "home-view").origin.x - content.x,
            EXPANDED - COLLAPSED,
            "the content moves over by the extra width"
        );

        shell.leave_rail(cx);
        assert_eq!(shell.rail_width(cx), EXPANDED, "it stays open off the rail");
        shell.click(cx, "rail-toggle");
        assert_eq!(shell.rail_width(cx), COLLAPSED, "the toggle closes it");
        assert_eq!(
            shell.bounds(cx, "home-view").origin,
            content,
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
    fn storage_tab_shows_the_torrent_cache_and_clears_it(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-storage");
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                gpui_kit::base::test_support::texts(window)
                    .iter()
                    .any(|text| text.visible() && text.text() == "84 bytes"),
                "Storage should display the current torrent cache size"
            );
        });

        shell.click(cx, "clear-torrent-cache");
        cx.run_until_parked();
        shell.act(cx, |window, _| {
            assert!(
                gpui_kit::base::test_support::texts(window)
                    .iter()
                    .any(|text| text.visible() && text.text() == "0 bytes"),
                "Storage should display the size after clear"
            );
        });
    }

    #[gpui_kit::test]
    fn the_disable_upload_toggle_saves_the_torrent_tab(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-storage");

        shell.click(cx, "setting-disable-upload");
        cx.run_until_parked();

        let saved = shell.torrent_saved.borrow();
        assert_eq!(saved.len(), 1, "one save for the toggle");
        assert!(saved[0].disable_upload, "upload is switched off");
        assert_eq!(
            saved[0].download_folder, None,
            "the folder keeps its default"
        );
    }

    #[gpui_kit::test]
    fn about_tab_credits_the_anime_offline_database(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-about");

        assert_eq!(
            shell.route(cx),
            Route::Settings {
                tab: SettingsTab::About
            }
        );
        shell.act(cx, |window, _| {
            let texts = gpui_kit::base::test_support::texts(window);
            assert!(
                texts
                    .iter()
                    .any(|text| text.visible() && text.text().contains("Open Database License")),
                "Settings → About credits the ODbL dataset"
            );
        });
    }

    #[gpui_kit::test]
    fn playback_tab_shows_the_player_choice_and_options(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-settings");
        shell.click(cx, "settings-playback");

        assert_eq!(
            shell.route(cx),
            Route::Settings {
                tab: SettingsTab::Playback
            }
        );
        shell.act(cx, |window, _| {
            assert!(window.try_find("settings-playback").is_some());
            let texts = gpui_kit::base::test_support::texts(window);
            for label in [
                "Player",
                "Watched threshold",
                "Resume where I stopped",
                "Auto-skip OP/ED",
            ] {
                assert!(
                    texts
                        .iter()
                        .any(|text| text.visible() && text.text().contains(label)),
                    "Settings → Playback shows {label:?}"
                );
            }
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
    fn a_list_transfer_preview_is_shown_in_the_shell_and_account_tab(cx: &mut TestAppContext) {
        use domain::{
            AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score,
            TransferChoice, TransferDirection, TransferItem, TransferPreview,
        };

        let shell = Shell::open_with_account(
            cx,
            AniListAccountState::Active {
                name: "FrierenFan".to_owned(),
                avatar: None,
                expires_at: std::time::UNIX_EPOCH + std::time::Duration::from_hours(500_000),
                expiring: false,
            },
        );
        let entry = ListEntry {
            media: MediaId::new(1),
            status: ListStatus::Current,
            score: Score::UNSCORED,
            progress: 12,
            progress_volumes: 0,
            repeat: 0,
            started_at: FuzzyDate::UNKNOWN,
            completed_at: FuzzyDate::UNKNOWN,
            notes: String::new(),
            private: false,
            priority: 0,
            advanced_scores: AdvancedScores::default(),
            custom_lists: Vec::new(),
            hidden_from_status_lists: false,
            created_at: std::time::UNIX_EPOCH,
            updated_at: std::time::UNIX_EPOCH,
        };
        let preview = TransferPreview {
            direction: TransferDirection::ToAniList,
            items: vec![TransferItem::new(MediaKind::Anime, entry, None)],
            custom_lists: Vec::new(),
        };
        let account = shell.view.read_with(cx, |shell, _| shell.account.clone());
        account.update(cx, |store, cx| {
            store.apply(AppEvent::TransferPreview(Some(preview)), cx);
        });

        shell.act(cx, |window, _| {
            assert!(
                window.try_find("transfer-banner-open").is_some(),
                "connecting offers the transfer outside Settings"
            );
        });
        shell.click(cx, "transfer-banner-open");
        shell.act(cx, |window, _| {
            let texts = gpui_kit::base::test_support::texts(window);
            assert!(
                texts
                    .iter()
                    .any(|text| text.visible()
                        && text.text().contains("Copy Local lists to AniList")),
                "the preview names the direction"
            );
            assert!(
                window.try_find("transfer-choice-1").is_some()
                    && window.try_find("transfer-confirm").is_some()
                    && window.try_find("transfer-cancel").is_some(),
                "each entry can be reviewed and the plan confirmed or dismissed"
            );
        });
        account.update(cx, |store, cx| {
            store.choose(MediaId::new(1), TransferChoice::Skip, cx);
        });
        let choices = account.read_with(cx, |store, _| match store.transfer_preview() {
            Some(preview) => preview
                .items
                .iter()
                .map(|item| (item.media, item.choice))
                .collect(),
            None => Vec::new(),
        });
        account.update(cx, |_, cx| {
            AccountStore::request(
                AccountCommand::TransferConfirm {
                    direction: TransferDirection::ToAniList,
                    choices,
                },
                cx,
            );
        });
        cx.run_until_parked();
        let commands = shell.commands();
        assert!(
            commands.iter().any(|command| matches!(
                command,
                AccountCommand::TransferConfirm { choices, .. }
                    if choices == &[(MediaId::new(1), TransferChoice::Skip)]
            )),
            "the overridden choice reaches the confirm command: {commands:?}"
        );
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
        assert!(shell.shows(cx, "rail-schedule"), "everything fits at 800px");
        assert!(
            !shell.shows(cx, "rail-more"),
            "no More while everything fits"
        );

        shell.resize(cx, 360.);
        assert!(shell.shows(cx, "rail-home"), "the top item stays");
        assert!(!shell.shows(cx, "rail-schedule"), "the bottom item folds");
        assert!(shell.shows(cx, "rail-more"), "More holds it");
        assert!(
            shell.shows(cx, "rail-settings"),
            "the system group never folds"
        );

        shell.resize(cx, 800.);
        assert!(shell.shows(cx, "rail-schedule"), "growing brings it back");
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

    /// The placeholder route shows every episode-card state in both layouts.
    #[gpui_kit::test]
    fn schedule_placeholder_shows_the_episode_gallery(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        shell.click(cx, "rail-schedule");
        assert_eq!(shell.route(cx), Route::Schedule);

        for id in [
            "mock-episode-card-0",
            "mock-episode-card-1",
            "mock-episode-card-2",
            "mock-episode-card-3",
            "mock-episode-card-4",
            "mock-episode-card-5",
            "mock-episode-row-0",
            "mock-episode-row-1",
            "mock-episode-row-2",
            "mock-episode-row-3",
            "mock-episode-row-4",
            "mock-episode-row-5",
        ] {
            assert!(shell.shows(cx, id), "{id} is drawn on the Schedule route");
        }
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

        shell.input(cx, "/manga");
        assert_eq!(shell.mode(cx), Mode::Navigate, "/manga is not a mode");
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
    fn the_discover_route_shows_its_view(cx: &mut TestAppContext) {
        let shell = Shell::open(cx);
        assert!(
            !shell.shows(cx, "discover-view"),
            "Home shows no Discover view"
        );

        shell.click(cx, "rail-discover");
        cx.run_until_parked();
        shell.act(cx, |_, _| {});

        assert!(
            shell.shows(cx, "discover-view"),
            "Discover renders its view"
        );
        assert!(
            shell.shows(cx, "discover-section-Popular Anime-card-0"),
            "the Discover loader's section renders a card"
        );
    }

    #[gpui_kit::test]
    fn discover_uses_the_account_name_and_follows_account_changes(cx: &mut TestAppContext) {
        let active = |name: &str| AniListAccountState::Active {
            name: name.to_owned(),
            avatar: None,
            expires_at: std::time::UNIX_EPOCH + Duration::from_hours(500_000),
            expiring: false,
        };
        let shell = Shell::open_with_account(cx, active("FrierenFan"));
        shell.click(cx, "rail-discover");
        cx.run_until_parked();
        assert_eq!(
            shell.users.borrow().as_slice(),
            [Some("FrierenFan".to_owned())],
            "the loader gets the AniList name, not an empty string"
        );

        let account = cx.read(|app| shell.view.read(app).account.clone());
        for (event, expected) in [
            (
                AppEvent::AccountChanged(active("Himmel")),
                Some("Himmel".to_owned()),
            ),
            (
                AppEvent::AccountChanged(AniListAccountState::LoggedOut),
                None,
            ),
        ] {
            cx.update(|cx| {
                account.update(cx, |account, cx| account.apply(event, cx));
            });
            cx.run_until_parked();
            assert_eq!(
                shell.users.borrow().last().cloned().flatten(),
                expected,
                "the loader follows the current account"
            );
        }
        assert_eq!(
            shell.users.borrow().len(),
            3,
            "each account change reloads exactly once"
        );
    }

    /// How the loopback AniList answers Search requests.
    #[derive(Clone, Copy)]
    enum SearchResponse {
        Success,
        Failure,
    }

    /// A shell whose Search screen talks to a loopback AniList that records
    /// every request and returns enough cards to scroll.
    struct SearchShell {
        window: WindowHandle<Root>,
        view: Entity<AppShell>,
        requests: Arc<AtomicUsize>,
        _runtime: tokio::runtime::Runtime,
        _dir: tempfile::TempDir,
    }

    impl SearchShell {
        fn open(cx: &mut TestAppContext) -> Self {
            Self::open_with_options(cx, 1, SearchResponse::Success)
        }

        fn open_with_pages(cx: &mut TestAppContext, pages: u32) -> Self {
            Self::open_with_options(cx, pages, SearchResponse::Success)
        }

        fn open_failing(cx: &mut TestAppContext) -> Self {
            Self::open_with_options(cx, 1, SearchResponse::Failure)
        }

        #[expect(
            clippy::too_many_lines,
            reason = "one test harness builds a mock AniList server and its window"
        )]
        fn open_with_options(
            cx: &mut TestAppContext,
            pages: u32,
            response: SearchResponse,
        ) -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let runtime = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .worker_threads(1)
                .build()
                .expect("runtime");

            let fixture: Value = serde_json::from_str(
                &std::fs::read_to_string(format!(
                    "{}/../metadata/tests/fixtures/search_frieren.json",
                    env!("CARGO_MANIFEST_DIR")
                ))
                .expect("fixture"),
            )
            .expect("valid JSON");
            let first = fixture["data"]["Page"]["media"][0].clone();

            let requests = Arc::new(AtomicUsize::new(0));
            let app = Router::new()
                .route(
                    "/",
                    post(
                        move |State((hits, pages, mode)): State<(
                            Arc<AtomicUsize>,
                            u32,
                            SearchResponse,
                        )>,
                              Json(request): Json<Value>| {
                            hits.fetch_add(1, Ordering::SeqCst);
                            let page = request["variables"]["page"].as_u64().unwrap_or(1);
                            let media: Vec<Value> = (0..120)
                                .map(|i| {
                                    let mut card = first.clone();
                                    card["id"] = json!(page * 10_000 + i);
                                    card
                                })
                                .collect();
                            let body = json!({
                                "data": {"Page": {
                                    "pageInfo": {"hasNextPage": page < u64::from(pages)},
                                    "media": media,
                                }}
                            });
                            async move {
                                if matches!(mode, SearchResponse::Failure) {
                                    (StatusCode::INTERNAL_SERVER_ERROR, Json(json!({})))
                                } else {
                                    (StatusCode::OK, Json(body))
                                }
                            }
                        },
                    ),
                )
                .with_state((Arc::clone(&requests), pages, response));
            let listener = runtime
                .block_on(TcpListener::bind("127.0.0.1:0"))
                .expect("bind");
            let address = listener.local_addr().expect("address");
            runtime.spawn(async move { axum::serve(listener, app).await.expect("serve") });

            let db = runtime
                .block_on(db::Db::open(&dir.path().join("autumn.db")))
                .expect("db");
            let http = net::Client::builder()
                .max_retries(0)
                .build()
                .expect("client");
            let anilist = metadata::AniList::with_endpoint(http, format!("http://{address}/"));
            let catalog = metadata::CatalogCache::new(
                anilist.clone(),
                net::PosterCache::new(dir.path()).expect("poster cache"),
            );
            let (window, view) = cx.update(|cx| {
                gpui_kit::init(cx);
                ui_theme::init(cx);
                ui_state::actions::bind_keys(cx);
                cx.set_reduce_motion(true);
                ui_state::bridge::init(cx, runtime.handle().clone());
                let options = WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(Bounds {
                        origin: point(px(0.), px(0.)),
                        size: size(px(1280.), px(800.)),
                    })),
                    ..WindowOptions::default()
                };
                gpui_kit::open_window(options, cx, |window, cx| {
                    let fetch_status = cx.new(|_| FetchStatus::default());
                    let account = cx.new(|_| AccountStore::default());
                    let entry_store = cx.new(|_| EntryStore::new(pending_services()));
                    let home_store = cx.new(|_| HomeStore::new(pending_services()));
                    cx.new(|cx| {
                        AppShell::new_with_palette_sources(
                            RailPrefs::default(),
                            settings::PlaybackSettings::default(),
                            crate::shell::PlaybackServices::inert(),
                            settings::LibrarySettings::default(),
                            crate::shell::LibraryServices::inert(),
                            TorrentSettings::default(),
                            TorrentServices::inert(),
                            cx.new(|_| {
                                ui_state::LibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| {
                                ui_state::ScanSummariesStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            cx.new(|_| {
                                ui_state::LibraryExplorerStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            fetch_status,
                            account,
                            "https://example.test/login".to_owned(),
                            |_, _| Task::ready(()),
                            |_, _| Task::ready(Ok(42)),
                            |_, _| Task::ready(()),
                            Rc::new(|_: Option<String>, _: &App| {
                                Task::ready(Ok::<_, BridgeError>(DiscoverUpdate::default()))
                            }),
                            entry_store,
                            cx.new(|_| {
                                ui_state::EntryLibraryStore::new(
                                    ui_state::test_support::pending_services(),
                                )
                            }),
                            home_store,
                            cx.new(|_| PlaybackStore::new(pending_services())),
                            settings::GeneralSettings {
                                onboarding_completed: true,
                            },
                            crate::shell::GeneralServices::inert(),
                            true,
                            db.clone(),
                            anilist,
                            catalog,
                            PlatformGateway::new(db),
                            window,
                            cx,
                        )
                    })
                })
                .expect("open window")
            });
            Self {
                window: window.downcast::<Root>().expect("root window"),
                view,
                requests,
                _runtime: runtime,
                _dir: dir,
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

        fn settle(&self, cx: &mut TestAppContext) {
            cx.executor().allow_parking();
            for _ in 0..20 {
                std::thread::sleep(std::time::Duration::from_millis(20));
                cx.run_until_parked();
                self.act(cx, |_, _| {});
            }
        }

        fn click(&self, cx: &mut TestAppContext, id: &'static str) {
            self.act(cx, |window, cx| window.click(id, cx));
        }

        fn fire_card_open(&self, cx: &mut TestAppContext, index: usize) {
            self.act(cx, |window, app| {
                let grid = self.view.read_with(app, |shell, app| {
                    shell
                        .search
                        .as_ref()
                        .map(|search| search.read_with(app, |search, _| search.grid_entity()))
                });
                if let Some(grid) = grid {
                    let open = grid.read_with(app, |grid, _| grid.open_action());
                    if let Some(open) = open {
                        open(index, window, app);
                    }
                }
            });
        }

        fn press(&self, cx: &mut TestAppContext, key: &'static str) {
            self.act(cx, |window, cx| window.press(key, cx));
        }

        fn grid_offset(&self, cx: &mut TestAppContext) -> Point<Pixels> {
            self.view.read_with(cx, |shell, cx| {
                shell.search.as_ref().map_or(Point::default(), |search| {
                    search.read_with(cx, SearchScreen::scroll_offset)
                })
            })
        }

        fn query_text(&self, cx: &mut TestAppContext) -> String {
            self.view.read_with(cx, |shell, cx| {
                shell.search.as_ref().map_or_else(String::new, |search| {
                    search.read_with(cx, SearchScreen::query_text)
                })
            })
        }

        fn set_query(&self, cx: &mut TestAppContext, text: &str) {
            self.view.update(cx, |shell, cx| {
                if let Some(search) = &shell.search {
                    search.update(cx, |search, cx| search.set_query_text(text, cx));
                }
            });
        }

        fn genres_state(&self, cx: &mut TestAppContext) -> (String, usize) {
            self.view.read_with(cx, |shell, cx| {
                shell.search.as_ref().map_or_else(
                    || (String::new(), 0),
                    |search| search.read_with(cx, SearchScreen::genres_text_and_cursor),
                )
            })
        }

        fn set_genres(&self, cx: &mut TestAppContext, text: &str, cursor: usize) {
            self.view.update(cx, |shell, cx| {
                if let Some(search) = &shell.search {
                    search.update(cx, |search, cx| search.set_genres_text(text, cursor, cx));
                }
            });
        }

        fn schedule_apply(&self, cx: &mut TestAppContext) {
            self.view.update(cx, |shell, cx| {
                if let Some(search) = &shell.search {
                    search.update(cx, SearchScreen::schedule_apply_now);
                }
            });
        }
    }

    #[gpui_kit::test]
    fn opening_the_shell_makes_no_search_request(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        shell.settle(cx);

        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            0,
            "creating the screen must not spend an AniList request"
        );
        assert_eq!(
            shell.view.read_with(cx, |shell, _| shell.route()),
            Route::Home,
            "startup must not jump to Search"
        );
    }

    #[gpui_kit::test]
    fn back_restores_the_search_grid_offset_and_text(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        let first = SearchFilters {
            query: Some("frieren".to_owned()),
            ..SearchFilters::default()
        };
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(first.clone()), cx);
        });
        shell.settle(cx);
        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            1,
            "first page fetched"
        );
        assert_eq!(
            shell.query_text(cx),
            "frieren",
            "the route's query is shown"
        );

        let saved = point(px(0.), px(-600.));
        shell.view.update(cx, |shell, cx| {
            if let Some(search) = &shell.search {
                search.update(cx, |search, cx| {
                    search.restore_scroll(saved, cx);
                });
            }
        });
        shell.settle(cx);
        assert_eq!(shell.grid_offset(cx), saved, "the grid is scrolled");

        // A filter button commits a second route, saving the grid offset.
        shell.click(cx, "search-sort");
        shell.settle(cx);
        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            2,
            "one request per change"
        );
        let second = shell.view.read_with(cx, |shell, _| shell.route());
        assert_ne!(
            second,
            Route::Search(first.clone()),
            "a new route was pushed"
        );

        // Typing a newer query commits a third route; its text is what Back
        // must replace with the older route's text.
        shell.set_query(cx, "vinland");
        shell.settle(cx);
        assert_eq!(shell.query_text(cx), "vinland");

        shell.press(cx, BACK_KEY);
        shell.settle(cx);

        assert_eq!(
            shell.view.read_with(cx, |shell, _| shell.route()),
            Route::Search(first.clone()),
            "Back restores the earlier filters"
        );
        assert_eq!(shell.grid_offset(cx), saved, "the grid offset comes back");
        assert_eq!(
            shell.query_text(cx),
            "frieren",
            "the old query is written back"
        );
        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            2,
            "Back restores cached results without another request"
        );
    }

    #[gpui_kit::test]
    fn scrolling_near_the_end_pages_without_a_button(cx: &mut TestAppContext) {
        let shell = SearchShell::open_with_pages(cx, 3);
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(SearchFilters::default()), cx);
        });
        shell.settle(cx);
        assert_eq!(shell.requests.load(Ordering::SeqCst), 1, "first page only");

        shell.act(cx, |window, cx| {
            assert!(
                window.try_find("search-more").is_none(),
                "paging is driven by scrolling, not a button"
            );
            window.scroll(
                "poster-grid",
                ScrollDelta::Pixels(point(px(0.), px(-5_200.))),
                cx,
            );
        });
        shell.settle(cx);

        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            2,
            "scrolling to the end fetched exactly one more page"
        );
    }

    #[gpui_kit::test]
    fn a_filter_button_folds_in_text_that_is_not_committed_yet(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(SearchFilters::default()), cx);
        });
        shell.settle(cx);

        // Text sitting in the input but not yet committed must survive the
        // next button click instead of being dropped when it replaces the task.
        shell.set_query(cx, "frieren");
        shell.click(cx, "search-sort");
        shell.settle(cx);

        let route = shell.view.read_with(cx, |shell, _| shell.route());
        assert!(
            matches!(route, Route::Search(_)),
            "the shell stayed on Search"
        );
        if let Route::Search(filters) = route {
            assert_eq!(
                filters.query.as_deref(),
                Some("frieren"),
                "the pending query is part of the committed filters"
            );
            assert!(
                filters.sort.is_some(),
                "the button's change was applied too"
            );
        }
    }

    #[gpui_kit::test]
    fn a_failed_first_load_shows_the_error(cx: &mut TestAppContext) {
        let shell = SearchShell::open_failing(cx);
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(SearchFilters::EMPTY), cx);
        });
        shell.settle(cx);

        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            1,
            "exactly one request was made"
        );
        let message = shell.view.read_with(cx, |shell, cx| {
            shell.search.as_ref().and_then(|search| {
                search.read_with(cx, |search, _| search.error_text().map(str::to_owned))
            })
        });
        assert!(
            message
                .as_deref()
                .is_some_and(|text| text.contains("HTTP 500")),
            "the cache-miss failure is kept: {message:?}"
        );
        let shown = cx
            .update_window(shell.window.into(), |_, window, cx| {
                window.render_frame(cx);
                window.find("search-error").visible()
            })
            .expect("window is open");
        assert!(shown, "a cache-miss failure is surfaced in the UI");
    }

    #[gpui_kit::test]
    fn a_typed_commit_leaves_the_genres_input_and_caret_alone(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(SearchFilters::EMPTY), cx);
        });
        shell.settle(cx);

        shell.set_genres(cx, "action,com,", 10);
        assert_eq!(shell.genres_state(cx), ("action,com,".to_owned(), 10));

        // The button folds in the pending text and navigates forward; the
        // restored filters must not rewrite what the user just typed.
        shell.click(cx, "search-sort");
        shell.settle(cx);

        assert_eq!(
            shell.genres_state(cx),
            ("action,com,".to_owned(), 10),
            "the unformatted text and caret survive the forward commit"
        );
    }

    #[gpui_kit::test]
    fn back_on_a_cache_hit_cancels_a_pending_typed_commit(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(SearchFilters::EMPTY), cx);
        });
        shell.settle(cx);
        shell.click(cx, "search-sort");
        shell.settle(cx);
        let requests = shell.requests.load(Ordering::SeqCst);

        shell.set_query(cx, "frieren");
        shell.schedule_apply(cx);
        // Back lands on a cached filter set before the debounce expires.
        shell.press(cx, BACK_KEY);
        shell.settle(cx);

        assert_eq!(
            shell.view.read_with(cx, |shell, _| shell.route()),
            Route::Search(SearchFilters::EMPTY),
            "Back wins over the pending typed commit"
        );
        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            requests,
            "the cancelled commit made no request"
        );
    }

    #[gpui_kit::test]
    fn opening_a_result_then_back_restores_the_search_filters_and_grid(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        let filters = SearchFilters {
            query: Some("frieren".to_owned()),
            ..SearchFilters::EMPTY
        };
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(filters.clone()), cx);
        });
        shell.settle(cx);
        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            1,
            "first page fetched"
        );

        let saved = point(px(0.), px(-600.));
        shell.view.update(cx, |shell, cx| {
            if let Some(search) = &shell.search {
                search.update(cx, |search, cx| search.restore_scroll(saved, cx));
            }
        });
        shell.settle(cx);
        assert_eq!(shell.grid_offset(cx), saved, "the grid is scrolled");

        // Open a result through the grid's Open callback, as the card's
        // context menu does, so the on_open wiring is exercised end to end.
        shell.fire_card_open(cx, 14);
        shell.settle(cx);

        assert_eq!(
            shell.view.read_with(cx, |shell, _| shell.route()),
            Route::Entry {
                id: 10_014,
                tab: None
            },
            "a result opens Entry with its AniList id"
        );

        shell.press(cx, BACK_KEY);
        shell.settle(cx);

        assert_eq!(
            shell.view.read_with(cx, |shell, _| shell.route()),
            Route::Search(filters.clone()),
            "Back restores the committed filters"
        );
        assert_eq!(shell.query_text(cx), "frieren", "the query comes back");
        assert_eq!(shell.grid_offset(cx), saved, "the grid offset comes back");
        assert_eq!(
            shell.requests.load(Ordering::SeqCst),
            1,
            "Back restores cached results without another request"
        );
    }

    #[gpui_kit::test]
    fn a_restored_manga_format_is_cleared(cx: &mut TestAppContext) {
        let shell = SearchShell::open(cx);
        let restored = SearchFilters {
            kind: Some(domain::MediaKind::Manga),
            format: Some(ui_state::SearchFormat::Manga),
            ..SearchFilters::EMPTY
        };
        shell.view.update(cx, |shell, cx| {
            shell.navigate(Route::Search(restored), cx);
        });
        shell.settle(cx);

        let committed = shell.view.read_with(cx, |shell, cx| {
            shell
                .search
                .as_ref()
                .map(|search| search.read(cx).filters().clone())
        });
        assert_eq!(
            committed,
            Some(SearchFilters::EMPTY),
            "the manga kind and format are dropped before the query is sent"
        );
    }
}
