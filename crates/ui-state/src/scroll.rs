//! Restores a route view's scroll offset, also after its content loads.
//!
//! Layout clamps a scroll offset to the content drawn in that frame. A
//! screen whose data arrives later draws short content first, so an offset
//! restored on entry is clamped to the top and lost. The shell keeps the
//! offset in a [`ScrollRestore`] and applies it again when the screen emits
//! [`ContentLoaded`]. See ARCHITECTURE §5.

use gpui_kit::{Pixels, Point, ScrollHandle};

/// Emitted by a screen once its asynchronously loaded content is rendered,
/// so the shell can re-apply the route's saved scroll offset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ContentLoaded;

/// A route view's scroll handle and the offset being restored on it.
#[derive(Debug, Default)]
pub struct ScrollRestore {
    handle: ScrollHandle,
    pending: Option<Point<Pixels>>,
}

impl ScrollRestore {
    /// A handle scrolled to the top, with nothing to restore.
    pub fn new() -> Self {
        Self::default()
    }

    /// The handle to pass to `track_scroll` on the route view.
    pub fn handle(&self) -> &ScrollHandle {
        &self.handle
    }

    /// The current scroll offset.
    pub fn offset(&self) -> Point<Pixels> {
        self.handle.offset()
    }

    /// Scrolls to `offset` and keeps it for [`Self::content_loaded`].
    pub fn restore(&mut self, offset: Point<Pixels>) {
        self.handle.set_offset(offset);
        self.pending = Some(offset);
    }

    /// Re-applies the offset given to the last [`Self::restore`], once.
    ///
    /// Call it when the screen on screen emits [`ContentLoaded`]. Later
    /// loads, such as a refresh, leave the scroll position alone.
    ///
    /// The pending offset belongs to the current route, so drop the
    /// subscription to a screen once you navigate away from it. If the user
    /// scrolls before the content arrives, this still jumps to the saved
    /// offset.
    pub fn content_loaded(&mut self) {
        if let Some(offset) = self.pending.take() {
            self.handle.set_offset(offset);
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui_kit::{
        AppContext, Bounds, Context, Entity, EventEmitter, InteractiveElement, IntoElement,
        ParentElement, Pixels, Point, Render, StatefulInteractiveElement, Styled, Subscription,
        TestAppContext, Window, WindowBounds, WindowOptions, div, point, px, size,
    };

    use super::{ContentLoaded, ScrollRestore};

    /// A screen whose rows arrive over a channel, like a network fetch.
    struct Screen {
        rows: usize,
    }

    impl EventEmitter<ContentLoaded> for Screen {}

    impl Screen {
        fn new(rows: flume::Receiver<usize>, cx: &mut Context<'_, Self>) -> Self {
            cx.spawn(async move |screen, cx| {
                if let Ok(rows) = rows.recv_async().await {
                    screen
                        .update(cx, |screen, cx| {
                            screen.rows = rows;
                            cx.emit(ContentLoaded);
                            cx.notify();
                        })
                        .ok();
                }
            })
            .detach();
            Self { rows: 0 }
        }
    }

    impl Render for Screen {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            div().children((0..self.rows).map(|row| div().h(px(48.)).child(format!("Row {row}"))))
        }
    }

    /// Stands in for the shell: owns the scroll and listens for the screen.
    struct Host {
        scroll: ScrollRestore,
        screen: Entity<Screen>,
        _loaded: Subscription,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<'_, Self>) -> impl IntoElement {
            div()
                .id("route-view")
                .size_full()
                .overflow_y_scroll()
                .track_scroll(self.scroll.handle())
                .child(self.screen.clone())
        }
    }

    fn at(y: f32) -> Point<Pixels> {
        point(px(0.), px(-y))
    }

    #[gpui_kit::test]
    fn offset_comes_back_once_async_content_arrives(cx: &mut TestAppContext) {
        let (send_rows, rows) = flume::bounded(1);
        let window = cx.update(|cx| {
            gpui_kit::init(cx);
            let options = WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(Bounds {
                    origin: point(px(0.), px(0.)),
                    size: size(px(800.), px(600.)),
                })),
                ..WindowOptions::default()
            };
            cx.open_window(options, |_, cx| {
                cx.new(|cx| {
                    let screen = cx.new(|cx| Screen::new(rows, cx));
                    let loaded =
                        cx.subscribe(&screen, |host: &mut Host, _, _: &ContentLoaded, _| {
                            host.scroll.content_loaded();
                        });
                    Host {
                        scroll: ScrollRestore::new(),
                        screen,
                        _loaded: loaded,
                    }
                })
            })
            .unwrap()
        });
        let offset = |cx: &mut TestAppContext| {
            window.update(cx, |_, window, _| window.refresh()).unwrap();
            cx.run_until_parked();
            window
                .read_with(cx, |host, _| host.scroll.offset())
                .unwrap()
        };

        // Returning to the route restores 600px before any row has loaded.
        window
            .update(cx, |host, _, _| host.scroll.restore(at(600.)))
            .unwrap();
        assert_eq!(
            offset(cx),
            at(0.),
            "empty content clamps the offset to the top"
        );

        send_rows.send(100).unwrap();
        assert_eq!(
            offset(cx),
            at(600.),
            "the offset is re-applied once rows arrive"
        );
    }
}
