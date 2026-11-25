//! The one playback session mirrored for the HUD and the Entry tab (#128).
//!
//! The store never drives the player itself: it applies engine events from
//! the foreground pump, and sends commands through the bridge so `render()`
//! stays pure (ARCHITECTURE §4).

use std::sync::Arc;

use domain::{AppEvent, MediaId, PlaybackState};
use engine::{EngineServices, PlayRequest};
use gpui_kit::{Context, Task, WeakEntity};

use crate::entry_store::call;

/// A playback failure the HUD can show, with Retry when it is transient.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaybackError {
    /// The media the failure concerns.
    pub media: MediaId,
    /// The episode the failure concerns.
    pub episode: u32,
    /// A message suitable for a toast.
    pub message: String,
    /// Whether a Retry action may succeed.
    pub retryable: bool,
}

/// The state of the active session, as last reported by the engine.
pub struct PlaybackStore {
    services: Arc<dyn EngineServices>,
    state: Option<PlaybackState>,
    error: Option<PlaybackError>,
    request: Option<Task<()>>,
}

impl std::fmt::Debug for PlaybackStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackStore")
            .field("state", &self.state)
            .field("error", &self.error)
            .finish_non_exhaustive()
    }
}

impl PlaybackStore {
    /// A store that sends playback commands through `services`.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services,
            state: None,
            error: None,
            request: None,
        }
    }

    /// The current position, or `None` when no session is active.
    #[must_use]
    pub fn state(&self) -> Option<PlaybackState> {
        self.state
    }

    /// The last failure, until it is dismissed or retried.
    #[must_use]
    pub fn error(&self) -> Option<&PlaybackError> {
        self.error.as_ref()
    }

    /// Applies an event from the foreground pump.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        match event {
            AppEvent::PlaybackState(state) => {
                self.state = Some(state);
                cx.notify();
            }
            AppEvent::PlaybackStopped => {
                self.state = None;
                cx.notify();
            }
            AppEvent::PlaybackError {
                media,
                episode,
                message,
                retryable,
            } => {
                self.error = Some(PlaybackError {
                    media,
                    episode,
                    message,
                    retryable,
                });
                cx.notify();
            }
            _ => {}
        }
    }

    /// Starts `request`; a newer request cancels the older one's task.
    pub fn play(&mut self, request: PlayRequest, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.request =
            Some(cx.spawn(
                async move |store, cx| match call(cx, services.play(&request)).await {
                    Ok(()) => {}
                    Err(message) => failed(
                        &store,
                        cx,
                        PlaybackError {
                            media: request.media,
                            episode: request.episode,
                            message,
                            retryable: true,
                        },
                    ),
                },
            ));
    }

    /// Stops the active session.
    pub fn stop(&mut self, cx: &mut Context<'_, Self>) {
        let services = Arc::clone(&self.services);
        self.request = Some(cx.spawn(async move |_, cx| {
            let _ = call(cx, services.stop()).await;
        }));
    }

    /// Retries the failure currently shown by the toast.
    pub fn retry(&mut self, cx: &mut Context<'_, Self>) {
        let Some(error) = self.error.take() else {
            return;
        };
        cx.notify();
        if !error.retryable {
            return;
        }
        let services = Arc::clone(&self.services);
        self.request = Some(cx.spawn(async move |store, cx| {
            match call(cx, services.retry_sync(error.media, error.episode)).await {
                Ok(()) => {}
                Err(message) => failed(
                    &store,
                    cx,
                    PlaybackError {
                        media: error.media,
                        episode: error.episode,
                        message,
                        retryable: true,
                    },
                ),
            }
        }));
    }
}

fn failed(store: &WeakEntity<PlaybackStore>, cx: &mut gpui_kit::AsyncApp, error: PlaybackError) {
    let _ = store.update(cx, |store, cx| {
        store.error = Some(error);
        cx.notify();
    });
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use domain::{AppEvent, MediaId, PlaybackState, StreamSource};
    use engine::PlayRequest;
    use gpui_kit::{AppContext, TestAppContext};
    use tokio::runtime::Builder;

    use crate::{
        bridge,
        test_support::{PlaybackCall, PlaybackServices, pending_services},
    };

    use super::PlaybackStore;

    #[gpui_kit::test]
    fn mirrors_the_session_and_clears_it_on_stop(cx: &mut TestAppContext) {
        let store = cx.new(|_| PlaybackStore::new(pending_services()));
        let state = PlaybackState {
            media: MediaId::new(7),
            episode: 3,
            position: Duration::from_secs(42),
            duration: Some(Duration::from_mins(24)),
            paused: false,
        };

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::PlaybackState(state), cx);
            });
        });
        assert_eq!(store.read_with(cx, |store, _| store.state()), Some(state));

        cx.update(|cx| {
            store.update(cx, |store, cx| store.apply(AppEvent::PlaybackStopped, cx));
        });
        assert_eq!(store.read_with(cx, |store, _| store.state()), None);
    }

    #[gpui_kit::test]
    fn mirrors_a_retryable_sync_error(cx: &mut TestAppContext) {
        let store = cx.new(|_| PlaybackStore::new(pending_services()));

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(
                    AppEvent::PlaybackError {
                        media: MediaId::new(7),
                        episode: 3,
                        message: "offline".to_owned(),
                        retryable: true,
                    },
                    cx,
                );
            });
        });

        let error = store.read_with(cx, |store, _| store.error().cloned());
        let error = error.expect("the toast has an error");
        assert!(error.retryable, "the toast can offer Retry");
        assert_eq!(error.message, "offline");
        assert_eq!(error.episode, 3);
    }

    #[gpui_kit::test]
    fn commands_reach_the_engine_and_retry_reuses_the_error(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = PlaybackServices::default();
        let calls = services.clone();
        let store = cx.new(|_| PlaybackStore::new(std::sync::Arc::new(services)));
        let request = PlayRequest::new(MediaId::new(2), 3, StreamSource::local("/media/ep.mkv"));

        cx.update(|cx| {
            store.update(cx, |store, cx| store.play(request.clone(), cx));
        });
        cx.run_until_parked();
        assert_eq!(calls.calls(), [PlaybackCall::Play(request)]);

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(
                    AppEvent::PlaybackError {
                        media: MediaId::new(2),
                        episode: 3,
                        message: "offline".to_owned(),
                        retryable: true,
                    },
                    cx,
                );
            });
            store.update(cx, PlaybackStore::retry);
        });
        cx.run_until_parked();
        assert_eq!(
            calls.calls().get(1),
            Some(&PlaybackCall::Retry(MediaId::new(2), 3))
        );
        assert_eq!(store.read_with(cx, |store, _| store.error().cloned()), None);

        cx.update(|cx| {
            store.update(cx, PlaybackStore::stop);
        });
        cx.run_until_parked();
        assert_eq!(calls.calls().get(2), Some(&PlaybackCall::Stop));
    }
}
