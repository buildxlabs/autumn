//! Autumn, a desktop app for anime and manga.

mod logging;
mod settings_store;
mod startup;

use std::{fmt::Display, process::ExitCode};

use gpui_kit::{
    App, AppContext, Bounds, Context, IntoElement, Render, Styled, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, px, size,
};

/// Initial width of the main window in logical pixels; the user can resize it.
const WINDOW_WIDTH: f32 = 1280.;
/// Initial height of the main window in logical pixels; the user can resize it.
const WINDOW_HEIGHT: f32 = 800.;

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

    gpui_kit::application().run(move |cx| {
        gpui_kit::init(cx);
        ui_state::bridge::init(cx, handle);
        // No store consumes an event yet; the first engine module adds one.
        ui_state::events::start_pump(cx, ui_events, |_event, _cx| {}).detach();
        if let Err(err) = open_main_window(cx) {
            // Logging is up, so this also reaches stderr.
            tracing::error!(error = %format_args!("{err:#}"), "failed to open the main window");
            cx.quit();
        }
    });
    ExitCode::SUCCESS
}

/// Opens the main window, centered on the primary display.
fn open_main_window(cx: &mut App) -> gpui_kit::Result<()> {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Autumn".into()),
            ..TitlebarOptions::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..WindowOptions::default()
    };
    gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| EmptyView))?;
    Ok(())
}

#[expect(
    clippy::print_stderr,
    reason = "a start that fails before or while logging is set up must still say why"
)]
fn report_fatal(err: &impl Display) {
    eprintln!("autumn: {err}");
}

/// Placeholder content until the app shell exists.
struct EmptyView;

impl Render for EmptyView {
    fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
        div().size_full()
    }
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
}
