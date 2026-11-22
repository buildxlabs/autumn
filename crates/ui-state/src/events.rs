//! Carries [`AppEvent`]s from the engine to the UI stores.
//!
//! The engine sends on a bounded [`flume`] channel from any thread. One
//! foreground task, started at boot with [`start_pump`], receives each event
//! and hands it to a router that updates the matching store. See
//! ARCHITECTURE §4.

use domain::AppEvent;
use gpui_kit::{App, Task};

/// How many events may wait in the channel before the engine's sends wait.
///
/// The engine coalesces high-frequency sources to at most 4 Hz, so the UI
/// falling this far behind means it is stalled, and back-pressure is right.
const EVENT_CAPACITY: usize = 256;

/// Publishes engine events to the foreground pump.
pub type Sender = flume::Sender<AppEvent>;

/// Creates the bounded engine-to-UI event channel.
pub fn channel() -> (Sender, flume::Receiver<AppEvent>) {
    flume::bounded(EVENT_CAPACITY)
}

/// Starts the foreground task that routes each received event to a store.
///
/// `route` runs on the main thread with the app context, so it can call
/// `entity.update(cx, …)`; the store then calls `cx.notify()`. The task
/// ends once every sender is dropped, and the app drops it on quit. Detach
/// it to keep it running for the app's lifetime.
pub fn start_pump(
    cx: &App,
    events: flume::Receiver<AppEvent>,
    mut route: impl FnMut(AppEvent, &mut App) + 'static,
) -> Task<()> {
    cx.spawn(async move |cx| {
        while let Ok(event) = events.recv_async().await {
            cx.update(|cx| route(event, cx));
        }
    })
}

#[cfg(test)]
mod tests {
    use std::{cell::Cell, rc::Rc};

    use domain::AppEvent;
    use gpui_kit::{AppContext, Context, TestAppContext};
    use tokio::runtime::Builder;

    use super::{channel, start_pump};

    /// A store that records the events routed to it.
    struct RecordingStore {
        seen: Vec<AppEvent>,
    }

    impl RecordingStore {
        fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
            self.seen.push(event);
            cx.notify();
        }
    }

    #[gpui_kit::test]
    async fn engine_event_reaches_the_store_and_notifies(cx: &mut TestAppContext) {
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        cx.executor().allow_parking();
        let store = cx.new(|_| RecordingStore { seen: Vec::new() });
        let notified = Rc::new(Cell::new(0));
        cx.update(|cx| {
            let notified = Rc::clone(&notified);
            cx.observe(&store, move |_, _| notified.set(notified.get() + 1))
                .detach();
        });
        let (sender, receiver) = channel();
        let pump = cx.update(|cx| {
            let store = store.clone();
            start_pump(cx, receiver, move |event, cx| {
                store.update(cx, |store, cx| store.apply(event, cx));
            })
        });

        // The engine side sends from a Tokio worker, then drops its sender,
        // which ends the pump once the event is routed.
        runtime
            .spawn(async move { sender.send_async(AppEvent::Placeholder).await })
            .await
            .unwrap()
            .unwrap();
        pump.await;

        let seen = store.read_with(cx, |store, _| store.seen.clone());
        assert_eq!(
            seen,
            [AppEvent::Placeholder],
            "the store must see the event"
        );
        assert_eq!(
            notified.get(),
            1,
            "the store must notify its observers once"
        );
    }
}
