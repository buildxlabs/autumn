//! Autumn, a desktop app for anime and manga.

mod account;
mod fetch_status;
mod http_client;
mod logging;
mod rail_prefs;
mod settings_store;
mod startup;
mod transfer;

use std::{fmt::Display, process::ExitCode, sync::Arc, time::SystemTime};

use auth::{AniListAuth, SystemKeychain, login_url};
use db::Db;
use domain::{AniListAccountState, AppEvent};
use gpui_kit::{
    App, AppContext, Bounds, Entity, Pixels, Task, TitlebarOptions, WindowBounds,
    WindowDecorations, WindowOptions, assets::AllAssets, component::TitleBar, point, px, size,
};
use metadata::{AniList, CatalogCache};
use tracker::{AniListPlatform, Local};
use ui_screens::{AppShell, PosterCacheAction, RailPrefs};
use ui_state::PlatformGateway;
use ui_state::account::{AccountCommand, AccountStore};
use ui_state::fetch_status::FetchStatus;
use ui_state::transfer::{TransferCommand, TransferStore};

/// Initial width of the main window in logical pixels; the user can resize it.
const WINDOW_WIDTH: f32 = 1280.;
/// Initial height of the main window in logical pixels; the user can resize it.
const WINDOW_HEIGHT: f32 = 800.;
/// Where the macOS traffic lights sit: centred vertically in the 40px titlebar.
const TRAFFIC_LIGHTS: (f32, f32) = (14., 13.);

fn main() -> ExitCode {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("autumn-engine")
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            report_fatal(&format_args!("failed to start the Tokio runtime: {err:#}"));
            return ExitCode::FAILURE;
        }
    };
    let started = match startup::run(&runtime) {
        Ok(started) => started,
        Err(err) => {
            report_fatal(&format_args!("startup failed: {err:#}"));
            return ExitCode::FAILURE;
        }
    };
    // One runtime: startup opened the database on it, and the bridge runs
    // UI-requested futures on it.
    let handle = runtime.handle().clone();
    // The one shared client: posters, the fetch-status feed and (in debug
    // builds) the fetch-status debug actions all go through it.
    let client = started.client;
    // The palette searches the Local platform's list and the public AniList
    // catalog through the shared HTTP client.
    let anilist = AniList::new(client.clone());
    // My Lists reads card text from the stale-first cache only, so it makes
    // no network requests while the Local platform is active.
    let catalog = CatalogCache::new(anilist.clone(), started.poster_cache.clone());
    let gateway = PlatformGateway::new(Local::new(started.db.clone()));
    let (engine_events, ui_events) = ui_state::events::channel();
    let session = AniListAuth::new(client.clone(), Arc::new(SystemKeychain));
    // Restore before the pump starts, so the first account event is seen.
    let restored = restore_account(&session, &handle, &gateway, &started.db, &anilist);
    let _ = engine_events.send(AppEvent::AccountChanged(restored));
    let transfer_service = transfer::Transfers::new(
        started.db.clone(),
        anilist.clone(),
        session.clone(),
        engine_events.clone(),
    );
    let account_request = account::request(
        session,
        gateway.clone(),
        started.db.clone(),
        anilist.clone(),
        engine_events.clone(),
        transfer_service.clone(),
    );
    let transfer_request = {
        let service = transfer_service;
        move |command: TransferCommand, cx: &mut App| service.request(command, cx)
    };
    handle.spawn({
        let client = client.clone();
        let events = engine_events.clone();
        async move { fetch_status::observe(&client, events).await }
    });
    let prefs = rail_prefs::from_settings(&started.settings);
    let storage_request = poster_cache_request(started.poster_cache.clone());
    let save_prefs = rail_prefs::saver(started.db.clone(), started.settings);

    let http_client =
        http_client::PosterHttpClient::new(client.clone(), started.poster_cache, handle.clone());
    gpui_kit::application()
        .with_assets(AllAssets)
        .with_http_client(Arc::new(http_client))
        .run(move |cx| {
            gpui_kit::init(cx);
            ui_state::bridge::init(cx, handle);
            ui_theme::init(cx);
            let fetch_status = cx.new(|_| FetchStatus::default());
            let account = cx.new(|_| AccountStore::default());
            let transfer_store = cx.new(|_| TransferStore::default());
            #[cfg(debug_assertions)]
            fetch_status::debug::bind(&fetch_status, client, engine_events, cx);
            ui_state::events::start_pump(cx, ui_events, {
                let store = fetch_status.clone();
                let account = account.clone();
                let transfer = transfer_store.clone();
                move |event, cx| {
                    store.update(cx, |store, cx| store.apply(event.clone(), cx));
                    account.update(cx, |store, cx| store.apply(event.clone(), cx));
                    transfer.update(cx, |store, cx| store.apply(event, cx));
                }
            })
            .detach();
            ui_state::actions::bind_keys(cx);
            if let Err(err) = open_main_window(
                prefs,
                fetch_status,
                account,
                login_url(),
                save_prefs,
                storage_request,
                account_request,
                transfer_store,
                transfer_request,
                started.db.clone(),
                anilist,
                catalog,
                gateway,
                cx,
            ) {
                // Logging is up, so this also reaches stderr.
                tracing::error!(error = %format_args!("{err:#}"), "failed to open the main window");
                cx.quit();
            }
        });
    ExitCode::SUCCESS
}

/// Restores the saved account and connects AniList as the active platform
/// before the event pump starts. Local entries stay untouched (ADR 0004).
fn restore_account(
    session: &AniListAuth,
    handle: &tokio::runtime::Handle,
    gateway: &PlatformGateway,
    db: &Db,
    anilist: &AniList,
) -> AniListAccountState {
    let restored = handle.block_on(session.restore(SystemTime::now()));
    if !matches!(restored, AniListAccountState::LoggedOut) {
        let name = account::name(&restored);
        if let Ok(Some(token)) = handle.block_on(session.token()) {
            gateway.connect_anilist(AniListPlatform::new(
                db.clone(),
                anilist.clone(),
                token,
                name,
            ));
        }
    }
    restored
}

/// The main window's options: the 40px transparent titlebar and centered
/// bounds. On Linux the window also requests client-side decorations.
fn main_window_options(bounds: Bounds<Pixels>) -> WindowOptions {
    WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Autumn".into()),
            appears_transparent: true,
            traffic_light_position: Some(point(px(TRAFFIC_LIGHTS.0), px(TRAFFIC_LIGHTS.1))),
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_decorations: main_window_decorations(),
        // Lets the titlebar drag the window and draw client-side controls.
        ..TitleBar::window_options()
    }
}

/// Client-side decorations on Linux, where the compositor otherwise draws
/// its own title bar above our 40px one; the platform default on other OSes.
fn main_window_decorations() -> Option<WindowDecorations> {
    if cfg!(target_os = "linux") {
        Some(WindowDecorations::Client)
    } else {
        None
    }
}

/// Opens the main window, centered on the primary display. The shell starts
/// with `prefs` and calls `save_prefs` when the user changes them.
#[expect(
    clippy::too_many_arguments,
    reason = "the window receives its engine collaborators once, at construction"
)]
fn open_main_window(
    prefs: RailPrefs,
    fetch_status: Entity<FetchStatus>,
    account: Entity<AccountStore>,
    login_url: String,
    save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
    storage_request: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
    account_request: impl Fn(AccountCommand, &mut App) -> Task<()> + 'static,
    transfer_store: Entity<TransferStore>,
    transfer_request: impl Fn(TransferCommand, &mut App) -> Task<()> + 'static,
    db: Db,
    anilist: AniList,
    catalog: CatalogCache,
    gateway: PlatformGateway,
    cx: &mut App,
) -> gpui_kit::Result<()> {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    let options = main_window_options(bounds);
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            AppShell::new_with_palette_sources(
                prefs,
                fetch_status,
                account,
                login_url,
                save_prefs,
                storage_request,
                account_request,
                transfer_store,
                transfer_request,
                db,
                anilist,
                catalog,
                gateway,
                window,
                cx,
            )
        })
    })?;
    Ok(())
}

fn poster_cache_request(
    cache: net::PosterCache,
) -> impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> {
    move |action, cx| {
        let cache = cache.clone();
        let task = ui_state::bridge::run(cx, async move {
            if action == PosterCacheAction::Clear {
                cache.clear().await?;
            }
            cache.size_bytes().await
        });
        cx.background_executor().spawn(async move {
            let result = task
                .await
                .map_err(anyhow::Error::from)
                .and_then(|result| result.map_err(Into::into));
            if let Err(error) = &result {
                tracing::warn!(?action, %error, "poster cache request failed");
            }
            result
        })
    }
}

#[expect(
    clippy::print_stderr,
    reason = "a start that fails before or while logging is set up must still say why"
)]
fn report_fatal(err: &impl Display) {
    eprintln!("autumn: {err}");
}

#[cfg(test)]
mod tests {
    use db::Db;
    use gpui_kit::{AppContext, Task, TestAppContext};
    use metadata::{AniList, CatalogCache};
    use tracker::Local;
    use ui_screens::RailPrefs;
    use ui_state::PlatformGateway;
    use ui_state::account::AccountStore;
    use ui_state::transfer::TransferStore;

    use super::{FetchStatus, open_main_window};

    #[gpui_kit::test]
    fn opens_one_window(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().expect("a temp directory");
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("a test runtime");
        let db = runtime
            .block_on(Db::open(&dir.path().join("palette.db")))
            .expect("a test database");
        let anilist = AniList::new(net::Client::new().expect("a test HTTP client"));
        let catalog = CatalogCache::new(
            anilist.clone(),
            net::PosterCache::new(dir.path()).expect("a test poster cache"),
        );
        cx.update(|cx| {
            gpui_kit::init(cx);
            let fetch_status = cx.new(|_| FetchStatus::default());
            let account = cx.new(|_| AccountStore::default());
            let transfer = cx.new(|_| TransferStore::default());
            open_main_window(
                RailPrefs::default(),
                fetch_status,
                account,
                "https://example.test/login".to_owned(),
                |_, _| Task::ready(()),
                |_, _| Task::ready(Ok(0)),
                |_, _| Task::ready(()),
                transfer,
                |_, _| Task::ready(()),
                db.clone(),
                anilist,
                catalog,
                PlatformGateway::new(Local::new(db)),
                cx,
            )
            .expect("the test platform opens windows");
        });

        assert_eq!(cx.windows().len(), 1, "expected exactly one window");
    }

    /// On Linux the window options request client-side decorations, so only
    /// our titlebar shows there.
    #[cfg(target_os = "linux")]
    #[test]
    fn main_window_options_request_client_decorations() {
        use gpui_kit::{Bounds, WindowDecorations, point, px, size};

        use super::{WINDOW_HEIGHT, WINDOW_WIDTH, main_window_options};

        let options = main_window_options(Bounds::new(
            point(px(0.), px(0.)),
            size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)),
        ));
        assert_eq!(options.window_decorations, Some(WindowDecorations::Client));
    }
}
