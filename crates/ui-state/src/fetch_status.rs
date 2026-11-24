//! Fetch state received through the app event pump.

use std::{
    collections::HashMap,
    time::{Duration, Instant, SystemTime},
};

use domain::{AniListWait, AppEvent, SectionFetch};
use gpui_kit::{Context, EventEmitter, Task};

const LOADER_DELAY: Duration = Duration::from_millis(300);

/// The section that hosts the Home poster content.
pub const HOME_POSTERS: &str = "home-posters";

/// A command from a view to the owner of its fetch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FetchCommand {
    /// Repeat the request for the given section.
    Retry(&'static str),
    /// Run a loopback fetch to exercise the status UI.
    #[cfg(debug_assertions)]
    Debug(DebugFetch),
}

/// Which response the loopback verifier should give.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum DebugFetch {
    /// Wait before returning 200.
    Slow,
    /// Return 429 with a Retry-After header.
    RateLimited,
    /// Return 403 like an AniList outage.
    Unavailable,
}

/// The recovery UI for a section with a fetch problem.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum SectionStatus {
    /// The request failed with a user-facing message.
    Failed(String),
    /// AniList returned a 403 outage.
    AniListUnavailable,
    /// The section can show cached content, with its age for the banner.
    OfflineCached(String),
    /// The section has no cached content.
    OfflineEmpty,
}

/// The shell's global request indicators and per-section fetch outcomes.
#[derive(Debug, Default)]
pub struct FetchStatus {
    loading: bool,
    loader_timer: Option<Task<()>>,
    countdown: Option<u64>,
    countdown_timer: Option<Task<()>>,
    sections: HashMap<&'static str, SectionStatus>,
}

impl EventEmitter<FetchCommand> for FetchStatus {}

impl FetchStatus {
    /// Whether any active request has reached the loader threshold.
    #[must_use]
    pub fn loading(&self) -> bool {
        self.loading
    }

    /// Whole seconds remaining, rounded up, until AniList can accept a request.
    #[must_use]
    pub fn countdown(&self) -> Option<u64> {
        self.countdown
    }

    /// The outcome to show in this section, if it has reported one.
    #[must_use]
    pub fn section(&self, section: &str) -> Option<&SectionStatus> {
        self.sections.get(section)
    }

    /// Forward a command to the owner of the request.
    pub fn request(command: FetchCommand, cx: &mut Context<'_, Self>) {
        cx.emit(command);
    }

    /// Apply an event from the foreground pump and notify observing views.
    pub fn apply(&mut self, event: AppEvent, cx: &mut Context<'_, Self>) {
        match event {
            AppEvent::FetchActivity(started) => self.update_loader(started, cx),
            AppEvent::AniListRateLimit(deadline) => self.update_countdown(deadline, cx),
            AppEvent::SectionFetch { section, state } => {
                let status = match state {
                    SectionFetch::Ready => None,
                    SectionFetch::Failed(message) => Some(SectionStatus::Failed(message)),
                    SectionFetch::AniListUnavailable => Some(SectionStatus::AniListUnavailable),
                    SectionFetch::Offline {
                        last_updated: Some(updated),
                    } => Some(SectionStatus::OfflineCached(cache_age(updated))),
                    SectionFetch::Offline { last_updated: None } => {
                        Some(SectionStatus::OfflineEmpty)
                    }
                    _ => return,
                };
                if let Some(status) = status {
                    self.sections.insert(section, status);
                } else {
                    self.sections.remove(section);
                }
                cx.notify();
            }
            _ => {}
        }
    }

    fn update_loader(&mut self, started: Option<Instant>, cx: &mut Context<'_, Self>) {
        let deadline = started.and_then(|started| started.checked_add(LOADER_DELAY));
        let now = cx.background_executor().now();
        self.loader_timer = deadline.filter(|&until| until > now).map(|until| {
            cx.spawn(async move |store, cx| {
                let executor = cx.background_executor();
                executor
                    .timer(until.saturating_duration_since(executor.now()))
                    .await;
                let _ = store.update(cx, |store, cx| store.refresh_loader(Some(until), cx));
            })
        });
        self.refresh_loader(deadline, cx);
    }
    fn refresh_loader(&mut self, deadline: Option<Instant>, cx: &mut Context<'_, Self>) {
        let loading = deadline.is_some_and(|until| until <= cx.background_executor().now());
        if self.loading != loading {
            self.loading = loading;
            cx.notify();
        }
    }

    fn update_countdown(&mut self, wait: Option<AniListWait>, cx: &mut Context<'_, Self>) {
        self.show_countdown(wait, cx);
        self.countdown_timer = wait
            .filter(|wait| wait.until > cx.background_executor().now())
            .map(|wait| {
                cx.spawn(async move |store, cx| {
                    loop {
                        let remaining = wait
                            .until
                            .saturating_duration_since(cx.background_executor().now());
                        if store
                            .update(cx, |store, cx| store.show_countdown(Some(wait), cx))
                            .is_err()
                            || remaining.is_zero()
                        {
                            break;
                        }

                        // Wake at the next whole second remaining.
                        let fraction = Duration::from_nanos(u64::from(remaining.subsec_nanos()));
                        let delay = if fraction.is_zero() {
                            Duration::from_secs(1)
                        } else {
                            fraction
                        };
                        cx.background_executor().timer(delay).await;
                    }
                })
            });
    }

    fn show_countdown(&mut self, wait: Option<AniListWait>, cx: &mut Context<'_, Self>) {
        let seconds = wait.and_then(|wait| {
            let remaining = wait
                .until
                .saturating_duration_since(cx.background_executor().now());
            (wait.queued || !remaining.is_zero())
                .then(|| remaining.as_secs() + u64::from(remaining.subsec_nanos() != 0))
        });
        if self.countdown != seconds {
            self.countdown = seconds;
            cx.notify();
        }
    }
}

/// How long ago `updated` was, for the offline banner: "just now",
/// "5 minutes ago", "2 days ago".
#[must_use]
pub fn cache_age(updated: SystemTime) -> String {
    let minutes = updated.elapsed().unwrap_or_default().as_secs() / 60;
    match minutes {
        0 => "just now".into(),
        1 => "1 minute ago".into(),
        2..60 => format!("{minutes} minutes ago"),
        60..120 => "1 hour ago".into(),
        120..1440 => format!("{} hours ago", minutes / 60),
        1440..2880 => "1 day ago".into(),
        _ => format!("{} days ago", minutes / 1440),
    }
}
