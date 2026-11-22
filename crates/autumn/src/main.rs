//! Autumn, a desktop app for anime and manga.

use std::fmt::Display;

use gpui_kit::{
    App, AppContext, Bounds, Context, IntoElement, Render, Styled, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, px, size,
};

/// Initial width of the main window in logical pixels; the user can resize it.
const WINDOW_WIDTH: f32 = 1280.;
/// Initial height of the main window in logical pixels; the user can resize it.
const WINDOW_HEIGHT: f32 = 800.;

fn main() {
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("autumn-engine")
        .build()
    {
        Ok(runtime) => runtime,
        Err(err) => {
            report_fatal("failed to start the Tokio runtime", &err);
            return;
        }
    };
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
            report_fatal("failed to open the main window", &err);
            cx.quit();
        }
    });
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
    reason = "no logger exists before the Phase 0 logging work; a failed start must say why"
)]
fn report_fatal(context: &str, err: &impl Display) {
    eprintln!("autumn: {context}: {err:#}");
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
