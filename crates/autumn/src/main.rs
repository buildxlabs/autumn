//! Autumn, a desktop app for anime and manga.

use std::fmt::Display;

use gpui_kit::{
    AnyWindowHandle, App, AppContext, Bounds, Context, IntoElement, Render, Styled,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, px, size,
};

/// Initial size of the main window, in logical pixels.
const WINDOW_WIDTH: f32 = 1280.;
const WINDOW_HEIGHT: f32 = 800.;

fn main() {
    gpui_kit::application().run(|cx| {
        gpui_kit::init(cx);
        if let Err(err) = open_main_window(cx) {
            report_fatal(&err);
            cx.quit();
        }
    });
}

/// Opens the main window, centered on the primary display.
fn open_main_window(cx: &mut App) -> gpui_kit::Result<AnyWindowHandle> {
    let bounds = Bounds::centered(None, size(px(WINDOW_WIDTH), px(WINDOW_HEIGHT)), cx);
    let options = WindowOptions {
        titlebar: Some(TitlebarOptions {
            title: Some("Autumn".into()),
            ..TitlebarOptions::default()
        }),
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        ..WindowOptions::default()
    };
    let (window, _) = gpui_kit::open_window(options, cx, |_, cx| cx.new(|_| EmptyView))?;
    Ok(window)
}

#[expect(
    clippy::print_stderr,
    reason = "no logger exists before the Phase 0 logging work; a failed start must say why"
)]
fn report_fatal(err: &impl Display) {
    eprintln!("autumn: failed to open the main window: {err:#}");
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
            open_main_window(cx).unwrap();
        });

        assert_eq!(cx.windows().len(), 1, "expected exactly one window");
    }
}
