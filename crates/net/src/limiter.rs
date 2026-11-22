//! Per-host token bucket and 429 lockout state.

use std::{collections::HashMap, sync::Arc, time::Duration};

use tokio::{
    sync::Mutex,
    time::{Instant, sleep},
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

/// A token bucket shared by every call to one host.
#[derive(Debug)]
pub(crate) struct HostLimiter {
    state: Mutex<State>,
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
            rate_per_sec: f64::from(config.requests_per_minute) / 60.0,
            burst,
        }
    }

    /// Wait until the host has capacity, then take one token.
    pub(crate) async fn acquire(&self) {
        loop {
            let wait = {
                let mut state = self.state.lock().await;
                let now = Instant::now();

                if let Some(until) = state.locked_until {
                    if until > now {
                        until - now
                    } else {
                        state.locked_until = None;
                        Duration::ZERO
                    }
                } else {
                    let elapsed = now.saturating_duration_since(state.last_refill);
                    state.tokens =
                        (state.tokens + elapsed.as_secs_f64() * self.rate_per_sec).min(self.burst);
                    state.last_refill = now;
                    if state.tokens >= 1.0 {
                        state.tokens -= 1.0;
                        return;
                    }
                    Duration::from_secs_f64((1.0 - state.tokens) / self.rate_per_sec)
                }
            };
            sleep(wait).await;
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
