//! Autumn, a desktop app for anime and manga.

mod fetch_status;
mod http_client;
mod logging;
mod rail_prefs;
mod settings_store;
mod startup;

use std::{fmt::Display, process::ExitCode, sync::Arc};

use gpui_kit::{
    App, AppContext, Bounds, Entity, Pixels, Task, TitlebarOptions, WindowBounds,
    WindowDecorations, WindowOptions, assets::AllAssets, component::TitleBar, point, px, size,
};
use ui_screens::{AppShell, PosterCacheAction, RailPrefs};
use ui_state::fetch_status::FetchStatus;

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
    let (engine_events, ui_events) = ui_state::events::channel();
    handle.spawn({
        let client = client.clone();
        let events = engine_events.clone();
        async move { fetch_status::observe(&client, events).await }
    });
    let prefs = rail_prefs::from_settings(&started.settings);
    let db = started.db.clone();
    let storage_request = poster_cache_request(started.poster_cache.clone());
    let save_prefs = rail_prefs::saver(started.db, started.settings);

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
            #[cfg(debug_assertions)]
            fetch_status::debug::bind(&fetch_status, client, engine_events, cx);
            ui_state::events::start_pump(cx, ui_events, {
                let store = fetch_status.clone();
                move |event, cx| store.update(cx, |store, cx| store.apply(event, cx))
            })
            .detach();
            ui_state::actions::bind_keys(cx);
            if let Err(err) =
                open_main_window(prefs, fetch_status, db, save_prefs, storage_request, cx)
            {
                // Logging is up, so this also reaches stderr.
                tracing::error!(error = %format_args!("{err:#}"), "failed to open the main window");
                cx.quit();
            }
        });
    ExitCode::SUCCESS
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
fn open_main_window(
    prefs: RailPrefs,
    fetch_status: Entity<FetchStatus>,
    db: db::Db,
    save_prefs: impl Fn(&RailPrefs, &mut App) -> Task<()> + 'static,
    storage_request: impl Fn(PosterCacheAction, &mut App) -> Task<gpui_kit::Result<u64>> + 'static,
    cx: &mut App,
) -> gpui_kit::Result<()> {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    let options = main_window_options(bounds);
    gpui_kit::open_window(options, cx, |window, cx| {
        cx.new(|cx| {
            AppShell::new(
                prefs,
                fetch_status,
                save_prefs,
                db,
                storage_request,
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
    use gpui_kit::{AppContext, Task, TestAppContext};
    use ui_screens::RailPrefs;

    use super::{FetchStatus, open_main_window};

    #[gpui_kit::test]
    fn opens_one_window(cx: &mut TestAppContext) {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let db = runtime
            .block_on(db::Db::open(&dir.path().join("autumn.db")))
            .unwrap();
        cx.update(|cx| {
            gpui_kit::init(cx);
            let fetch_status = cx.new(|_| FetchStatus::default());
            open_main_window(
                RailPrefs::default(),
                fetch_status,
                db,
                |_, _| Task::ready(()),
                |_, _| Task::ready(Ok(0)),
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
