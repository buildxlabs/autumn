//! Autumn, a desktop app for anime and manga.

mod logging;
mod settings_store;
mod startup;

use std::{fmt::Display, process::ExitCode};

use gpui_kit::{
    App, AppContext, Bounds, Pixels, TitlebarOptions, WindowBounds, WindowDecorations,
    WindowOptions, assets::AllAssets, component::TitleBar, point, px, size,
};
use ui_screens::AppShell;

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
    let _started = match startup::run(&runtime) {
        Ok(started) => started,
        Err(err) => {
            report_fatal(&format_args!("startup failed: {err:#}"));
            return ExitCode::FAILURE;
        }
    };
    // One runtime: startup opened the database on it, and the bridge runs
    // UI-requested futures on it.
    let handle = runtime.handle().clone();
    // The engine takes this sender once it exists (Phase 2). Holding it here
    // keeps the event pump alive for the whole run.
    let (_engine_events, ui_events) = ui_state::events::channel();

    gpui_kit::application()
        .with_assets(AllAssets)
        .run(move |cx| {
            gpui_kit::init(cx);
            ui_state::bridge::init(cx, handle);
            ui_theme::init(cx);
            // No store consumes an event yet; the first engine module adds one.
            ui_state::events::start_pump(cx, ui_events, |_event, _cx| {}).detach();
            ui_state::actions::bind_keys(cx);
            if let Err(err) = open_main_window(cx) {
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

/// Opens the main window, centered on the primary display.
fn open_main_window(cx: &mut App) -> gpui_kit::Result<()> {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    gpui_kit::open_window(main_window_options(bounds), cx, |window, cx| {
        cx.new(|cx| AppShell::new(window, cx))
    })?;
    Ok(())
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
    use gpui_kit::TestAppContext;

    use super::open_main_window;

    #[gpui_kit::test]
    fn opens_one_window(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            open_main_window(cx).expect("the test platform opens windows");
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
