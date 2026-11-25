//! The one playback session mirrored for the HUD and the Entry tab (#128).
//!
//! The store never drives the player itself: it applies engine events from
//! the foreground pump, and sends commands through the bridge so `render()`
//! stays pure (ARCHITECTURE §4).

use std::{sync::Arc, time::Duration};

use domain::{AppEvent, MediaId, PlaybackControl, PlaybackDetails, PlaybackState};
use engine::{EngineServices, PlayRequest};
use gpui_kit::{Context, Task, WeakEntity};

use crate::entry_store::call;

/// How long the HUD shows "Player closed" after the player quits on its own.
const CLOSED_NOTICE: Duration = Duration::from_secs(3);

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
    details: Option<PlaybackDetails>,
    error: Option<PlaybackError>,
    request: Option<Task<()>>,
    control: Option<Task<()>>,
    closed: bool,
    closed_task: Option<Task<()>>,
}

impl std::fmt::Debug for PlaybackStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlaybackStore")
            .field("state", &self.state)
            .field("details", &self.details)
            .field("error", &self.error)
            .field("closed", &self.closed)
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
            details: None,
            error: None,
            request: None,
            control: None,
            closed: false,
            closed_task: None,
        }
    }

    /// The current position, or `None` when no session is active.
    #[must_use]
    pub fn state(&self) -> Option<PlaybackState> {
        self.state
    }

    /// The labels supplied with the play request, when it had any.
    #[must_use]
    pub fn details(&self) -> Option<&PlaybackDetails> {
        self.details.as_ref()
    }

    /// The last failure, until it is dismissed or retried.
    #[must_use]
    pub fn error(&self) -> Option<&PlaybackError> {
        self.error.as_ref()
    }

    /// Whether the HUD should show "Player closed" after an unexpected quit.
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// Applies an event from the foreground pump.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        match event {
            AppEvent::PlaybackState(state) => {
                self.state = Some(state);
                cx.notify();
            }
            AppEvent::PlaybackStopped { unexpected } => {
                self.state = None;
                self.details = None;
                self.closed_task = None;
                self.closed = unexpected;
                if unexpected {
                    let store = cx.entity().downgrade();
                    self.closed_task = Some(cx.spawn(async move |_, cx| {
                        cx.background_executor().timer(CLOSED_NOTICE).await;
                        let _ = store.update(cx, |store, cx| {
                            store.closed = false;
                            store.closed_task = None;
                            cx.notify();
                        });
                    }));
                }
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
        self.details.clone_from(&request.details);
        self.closed = false;
        self.closed_task = None;
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
        self.closed = false;
        self.closed_task = None;
        let services = Arc::clone(&self.services);
        self.request = Some(cx.spawn(async move |_, cx| {
            let _ = call(cx, services.stop()).await;
        }));
    }

    /// Toggles play/pause from the HUD. The player's next event confirms it.
    pub fn toggle_pause(&mut self, cx: &mut Context<'_, Self>) {
        let Some(state) = self.state else {
            return;
        };
        let state = if state.paused {
            PlaybackControl::Resume
        } else {
            PlaybackControl::Pause
        };
        self.command(cx, move |services| services.set_playback_state(state));
    }

    /// Seeks to an absolute position in seconds.
    pub fn seek_to(&mut self, seconds: f64, cx: &mut Context<'_, Self>) {
        let seconds = if seconds.is_finite() && seconds > 0.0 {
            seconds
        } else {
            0.0
        };
        let seconds = self
            .state
            .and_then(|state| state.duration)
            .map_or(seconds, |duration| seconds.min(duration.as_secs_f64()));
        self.command(cx, move |services| services.seek(seconds));
    }

    /// Seeks by `delta` seconds from the current position.
    pub fn seek_by(&mut self, delta: f64, cx: &mut Context<'_, Self>) {
        let Some(state) = self.state else {
            return;
        };
        let seconds = state.position.as_secs_f64() + delta;
        self.seek_to(seconds, cx);
    }

    /// Seeks to a fraction of the file, from a click on the seek bar.
    pub fn seek_fraction(&mut self, fraction: f32, cx: &mut Context<'_, Self>) {
        let Some(duration) = self.state.and_then(|state| state.duration) else {
            return;
        };
        let seconds = f64::from(fraction.clamp(0.0, 1.0)) * duration.as_secs_f64();
        self.seek_to(seconds, cx);
    }

    /// Runs one HUD command, cancelling the previous one still in flight.
    fn command(
        &mut self,
        cx: &mut Context<'_, Self>,
        command: impl Fn(&Arc<dyn EngineServices>) -> engine::BoxFuture<Result<(), engine::EngineError>>
        + Send
        + 'static,
    ) {
        let services = Arc::clone(&self.services);
        self.control = Some(cx.spawn(async move |_, cx| {
            let _ = call(cx, command(&services)).await;
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

    use domain::{
        AppEvent, MediaId, PlaybackControl, PlaybackDetails, PlaybackState, StreamSource,
    };
    use engine::PlayRequest;
    use gpui_kit::{AppContext, TestAppContext};
    use tokio::runtime::Builder;

    use crate::{
        bridge,
        test_support::{PlaybackCall, PlaybackServices, pending_services},
    };

    use super::PlaybackStore;

    fn state() -> PlaybackState {
        PlaybackState {
            media: MediaId::new(7),
            episode: 3,
            position: Duration::from_secs(42),
            duration: Some(Duration::from_mins(24)),
            paused: false,
        }
    }

    #[gpui_kit::test]
    fn mirrors_the_session_and_clears_it_on_stop(cx: &mut TestAppContext) {
        let store = cx.new(|_| PlaybackStore::new(pending_services()));
        let state = state();

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::PlaybackState(state), cx);
            });
        });
        assert_eq!(store.read_with(cx, |store, _| store.state()), Some(state));

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::PlaybackStopped { unexpected: false }, cx);
            });
        });
        assert_eq!(store.read_with(cx, |store, _| store.state()), None);
        assert!(!store.read_with(cx, |store, _| store.closed()));
    }

    #[gpui_kit::test]
    fn an_unexpected_quit_shows_a_brief_notice(cx: &mut TestAppContext) {
        let store = cx.new(|_| PlaybackStore::new(pending_services()));
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::PlaybackState(state()), cx);
                store.apply(AppEvent::PlaybackStopped { unexpected: true }, cx);
            });
        });
        assert!(store.read_with(cx, |store, _| store.closed()));
        assert_eq!(store.read_with(cx, |store, _| store.state()), None);

        cx.executor().advance_clock(Duration::from_secs(3));
        cx.run_until_parked();
        assert!(!store.read_with(cx, |store, _| store.closed()));
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
        let details = PlaybackDetails::new("A show", Some("The beginning".to_owned()), None);
        let request = PlayRequest::new(MediaId::new(2), 3, StreamSource::local("/media/ep.mkv"))
            .with_details(details.clone());

        cx.update(|cx| {
            store.update(cx, |store, cx| store.play(request.clone(), cx));
        });
        cx.run_until_parked();
        assert_eq!(calls.calls(), [PlaybackCall::Play(request)]);
        assert_eq!(
            store.read_with(cx, |store, _| store.details().cloned()),
            Some(details)
        );

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

    #[gpui_kit::test]
    fn hud_commands_go_through_the_bridge(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let services = PlaybackServices::default();
        let calls = services.clone();
        let store = cx.new(|_| PlaybackStore::new(std::sync::Arc::new(services)));

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply(AppEvent::PlaybackState(state()), cx);
                store.toggle_pause(cx);
            });
        });
        cx.run_until_parked();
        assert_eq!(calls.calls(), [PlaybackCall::Pause(PlaybackControl::Pause)]);

        cx.update(|cx| {
            store.update(cx, |store, cx| store.seek_fraction(0.5, cx));
        });
        cx.run_until_parked();
        assert_eq!(calls.calls().get(1), Some(&PlaybackCall::Seek(720.0)));

        cx.update(|cx| {
            store.update(cx, |store, cx| store.seek_by(-10.0, cx));
        });
        cx.run_until_parked();
        assert_eq!(calls.calls().get(2), Some(&PlaybackCall::Seek(32.0)));
    }
}
