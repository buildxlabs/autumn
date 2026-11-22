//! Per-host token bucket and 429 lockout state.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{
    sync::{Mutex, watch},
    time::Instant,
};

use crate::{HostKind, Lockout};

/// Rate-limit settings for one host.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HostConfig {
    pub(crate) kind: HostKind,
    pub(crate) requests_per_minute: u32,
    pub(crate) burst: u32,
}

#[derive(Debug)]
struct State {
    tokens: f64,
    last_refill: Instant,
    locked_until: Option<Instant>,
}

#[derive(Debug, Default)]
struct WaitState {
    waiters: usize,
    next_slot: Option<Instant>,
    locked_until: Option<Instant>,
}

/// A snapshot of a host's active queue or lockout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct RateLimitSnapshot {
    /// When the next request may start.
    pub until: std::time::Instant,
    /// At least one request is still waiting.
    pub queued: bool,
}

/// Observes the wait state for a host's limiter.
#[derive(Debug)]
pub struct RateLimit(watch::Receiver<WaitState>);

impl RateLimit {
    /// Wait for the limiter state to change. Returns false if the client was dropped.
    pub async fn changed(&mut self) -> bool {
        self.0.changed().await.is_ok()
    }

    /// The active queue or lockout, including a queue whose timer is waking.
    #[must_use]
    pub fn snapshot(&mut self) -> Option<RateLimitSnapshot> {
        let now = Instant::now();
        let status = self.0.borrow_and_update();
        let queued = status.waiters > 0;
        let until = status
            .locked_until
            .filter(|&until| until > now)
            .max(status.next_slot.filter(|_| queued))?;
        Some(RateLimitSnapshot {
            until: until.into_std(),
            queued,
        })
    }
}

/// Keeps queue observation correct when a waiting request is cancelled.
struct Queued<'a>(&'a watch::Sender<WaitState>);

impl Drop for Queued<'_> {
    fn drop(&mut self) {
        self.0.send_modify(|status| {
            status.waiters -= 1;
            if status.waiters == 0 {
                status.next_slot = None;
            }
        });
    }
}

/// A token bucket shared by every call to one host.
#[derive(Debug)]
pub(crate) struct HostLimiter {
    state: Mutex<State>,
    status: watch::Sender<WaitState>,
    rate_per_sec: f64,
    burst: f64,
}

impl HostLimiter {
    pub(crate) fn new(config: HostConfig) -> Self {
        let burst = f64::from(config.burst);
        Self {
            state: Mutex::new(State {
                tokens: burst,
                last_refill: Instant::now(),
                locked_until: None,
            }),
            status: watch::Sender::new(WaitState::default()),
            rate_per_sec: f64::from(config.requests_per_minute) / 60.0,
            burst,
        }
    }

    pub(crate) fn subscribe(&self) -> RateLimit {
        RateLimit(self.status.subscribe())
    }

    /// Wait until the host has capacity, then take one token.
    pub(crate) async fn acquire(&self) {
        let mut queued = None;
        loop {
            let until = {
                let mut state = self.state.lock().await;
                let now = Instant::now();

                let until = if let Some(until) = state.locked_until.filter(|&until| until > now) {
                    until
                } else {
                    state.locked_until = None;
                    let elapsed = now.saturating_duration_since(state.last_refill);
                    state.tokens =
                        (state.tokens + elapsed.as_secs_f64() * self.rate_per_sec).min(self.burst);
                    state.last_refill = now;
                    if state.tokens >= 1.0 {
                        state.tokens -= 1.0;
                        return;
                    }
                    now + Duration::from_secs_f64((1.0 - state.tokens) / self.rate_per_sec)
                };

                self.status.send_modify(|status| {
                    if queued.is_none() {
                        status.waiters += 1;
                        queued = Some(Queued(&self.status));
                    }
                    status.next_slot = Some(until);
                    status.locked_until = state.locked_until;
                });
                until
            };
            tokio::time::sleep_until(until).await;
        }
    }

    /// Extend the host's lockout so it ends no earlier than `duration` from now.
    pub(crate) async fn pause(&self, duration: Duration) {
        let mut state = self.state.lock().await;
        let until = Instant::now() + duration;
        state.locked_until = Some(
            state
                .locked_until
                .map_or(until, |current| current.max(until)),
        );
        self.status
            .send_modify(|status| status.locked_until = state.locked_until);
    }

    pub(crate) async fn lockout(&self) -> Option<Lockout> {
        let mut state = self.state.lock().await;
        match state.locked_until {
            Some(until) if until > Instant::now() => Some(Lockout::new(until)),
            Some(_) => {
                state.locked_until = None;
                None
            }
            None => None,
        }
    }
}

#[derive(Debug)]
struct Entry {
    kind: HostKind,
    limiter: Arc<HostLimiter>,
}

/// The registered hosts, keyed by lowercased host name.
#[derive(Debug, Default)]
pub(crate) struct HostLimiters {
    hosts: HashMap<String, Entry>,
}

impl HostLimiters {
    pub(crate) fn insert(&mut self, host: String, config: HostConfig) {
        let kind = config.kind;
        self.hosts.insert(
            host,
            Entry {
                kind,
                limiter: Arc::new(HostLimiter::new(config)),
            },
        );
    }

    pub(crate) fn get(&self, host: &str) -> Option<Arc<HostLimiter>> {
        self.hosts.get(host).map(|entry| Arc::clone(&entry.limiter))
    }

    pub(crate) fn kind(&self, host: &str) -> Option<HostKind> {
        self.hosts.get(host).map(|entry| entry.kind)
    }
}
