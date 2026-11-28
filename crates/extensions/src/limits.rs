//! Fixed per-call caps and the kills that enforce them (ADR 0014, #269 §1–2).
//!
//! Luau runs synchronously inside the poll of its call future, so the actor
//! task cannot stop it. A single watchdog thread checks every running call
//! and arms its VM's one-shot interrupt once a call is over budget,
//! cancelled or stopped. The interrupt then raises at every safepoint until
//! the call unwinds, so `pcall` cannot swallow a kill.
//!
//! The watchdog is an OS thread rather than a Tokio task: extensions that spin
//! on every Tokio worker would otherwise starve the task meant to kill them.

use std::{
    future::Future,
    pin::Pin,
    sync::{
        Arc, Condvar, Mutex, OnceLock, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
    },
    task::{Context, Poll},
    time::{Duration, Instant},
};

use mlua::InterruptHandle;
use tokio::sync::watch;

use crate::error::ExtensionError;

/// How often the watchdog checks running calls.
const TICK: Duration = Duration::from_millis(10);

/// The caps every call runs under. Fixed in Phase 4; tests shorten them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Limits {
    /// Memory per VM, in bytes.
    pub(crate) memory: usize,
    /// Active Luau time per call. The clock pauses while the call waits on a
    /// host future.
    pub(crate) cpu: Duration,
    /// Wall-clock time per call, host futures included.
    pub(crate) wall: Duration,
}

impl Limits {
    /// The Phase 4 caps: 32 MiB, 1 s of CPU, 30 s of wall time.
    pub(crate) const PHASE_4: Self = Self {
        memory: 32 * 1024 * 1024,
        cpu: Duration::from_secs(1),
        wall: Duration::from_secs(30),
    };
}

/// Why the host killed a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kill {
    Cpu,
    Wall,
    Memory,
    /// Raised by `autumn.http.fetch` on the request after the cap.
    Requests,
    Cancelled,
}

impl Kill {
    /// The message the interrupt raises inside Luau.
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::Cpu => "killed: cpu_limit",
            Self::Wall => "killed: wall_timeout",
            Self::Memory => "killed: memory_limit",
            Self::Requests => "killed: request_limit",
            Self::Cancelled => "killed: cancelled",
        }
    }
}

impl From<Kill> for ExtensionError {
    fn from(kill: Kill) -> Self {
        match kill {
            Kill::Cpu => Self::CpuLimit,
            Kill::Wall => Self::WallTimeout,
            Kill::Memory => Self::MemoryLimit,
            Kill::Requests => Self::RequestLimit,
            Kill::Cancelled => Self::Cancelled,
        }
    }
}

/// One VM's kill switch. A VM is dropped after its first kill, so the
/// switch never resets.
#[derive(Debug, Default)]
pub(crate) struct KillSwitch {
    reason: Mutex<Option<Kill>>,
    interrupt: OnceLock<InterruptHandle>,
}

impl KillSwitch {
    /// Attaches the VM's interrupt; later calls are ignored.
    pub(crate) fn attach(&self, interrupt: InterruptHandle) {
        let _ = self.interrupt.set(interrupt);
    }

    /// Records the first kill and arms the interrupt.
    pub(crate) fn kill(&self, kill: Kill) {
        let mut reason = self.reason.lock().unwrap_or_else(PoisonError::into_inner);
        reason.get_or_insert(kill);
        drop(reason);
        self.rearm();
    }

    /// Arms the interrupt again so the next safepoint raises too.
    pub(crate) fn rearm(&self) {
        if let Some(interrupt) = self.interrupt.get() {
            interrupt.arm();
        }
    }

    /// The kill, if the host decided on one.
    pub(crate) fn reason(&self) -> Option<Kill> {
        *self.reason.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// What the watchdog knows about one running call.
#[derive(Debug)]
pub(crate) struct Watch {
    switch: Arc<KillSwitch>,
    limits: Limits,
    wall_deadline: Instant,
    cpu: Mutex<CpuClock>,
    caller_gone: Arc<AtomicBool>,
    stopping: watch::Receiver<bool>,
}

#[derive(Debug, Default)]
struct CpuClock {
    used: Duration,
    running_since: Option<Instant>,
}

impl Watch {
    /// Starts watching a call and registers it with the watchdog. It stops
    /// being watched when the returned `Arc` is dropped.
    ///
    /// # Errors
    ///
    /// [`ExtensionError::Vm`] when the watchdog thread cannot be started.
    pub(crate) fn start(
        switch: Arc<KillSwitch>,
        limits: Limits,
        caller_gone: Arc<AtomicBool>,
        stopping: watch::Receiver<bool>,
    ) -> Result<Arc<Self>, ExtensionError> {
        let watch = Arc::new(Self {
            switch,
            limits,
            wall_deadline: Instant::now() + limits.wall,
            cpu: Mutex::default(),
            caller_gone,
            stopping,
        });
        Watchdog::global()?.register(&watch);
        Ok(watch)
    }

    /// Kills the call if it is over budget, its caller left or its extension
    /// is stopping.
    fn check(&self, now: Instant) {
        let kill = if self.caller_gone.load(Ordering::Acquire) || *self.stopping.borrow() {
            Kill::Cancelled
        } else if self.cpu_used(now) >= self.limits.cpu {
            Kill::Cpu
        } else if now >= self.wall_deadline {
            Kill::Wall
        } else {
            return;
        };
        self.switch.kill(kill);
    }

    fn cpu_used(&self, now: Instant) -> Duration {
        let clock = self.cpu.lock().unwrap_or_else(PoisonError::into_inner);
        let running = clock
            .running_since
            .map_or(Duration::ZERO, |since| now.saturating_duration_since(since));
        clock.used + running
    }

    fn resume(&self) {
        self.cpu
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .running_since = Some(Instant::now());
    }

    fn suspend(&self) {
        let mut clock = self.cpu.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(since) = clock.running_since.take() {
            clock.used += since.elapsed();
        }
    }
}

/// A future whose time inside `poll` counts as the call's active CPU time.
pub(crate) struct Metered<'watch, F> {
    watch: &'watch Watch,
    inner: Pin<Box<F>>,
}

impl<'watch, F: Future> Metered<'watch, F> {
    pub(crate) fn new(watch: &'watch Watch, inner: F) -> Self {
        Self {
            watch,
            inner: Box::pin(inner),
        }
    }
}

impl<F: Future> Future for Metered<'_, F> {
    type Output = F::Output;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<F::Output> {
        let watch = self.watch;
        watch.resume();
        let poll = self.inner.as_mut().poll(cx);
        watch.suspend();
        poll
    }
}

/// The one thread that checks every running call.
#[derive(Debug, Default)]
struct Watchdog {
    calls: Mutex<Vec<Weak<Watch>>>,
    wake: Condvar,
}

impl Watchdog {
    fn global() -> Result<&'static Self, ExtensionError> {
        static WATCHDOG: OnceLock<Result<&'static Watchdog, String>> = OnceLock::new();
        WATCHDOG
            .get_or_init(|| {
                let watchdog: &'static Self = Box::leak(Box::default());
                std::thread::Builder::new()
                    .name("extension-watchdog".to_owned())
                    .spawn(|| watchdog.run())
                    .map(|_| watchdog)
                    .map_err(|error| format!("cannot start the extension watchdog: {error}"))
            })
            .clone()
            .map_err(ExtensionError::Vm)
    }

    fn register(&self, watch: &Arc<Watch>) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(Arc::downgrade(watch));
        self.wake.notify_one();
    }

    fn run(&self) {
        let mut calls = self.calls.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            calls.retain(|call| call.strong_count() > 0);
            calls = if calls.is_empty() {
                self.wake
                    .wait(calls)
                    .unwrap_or_else(PoisonError::into_inner)
            } else {
                self.wake
                    .wait_timeout(calls, TICK)
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            };
            let now = Instant::now();
            for call in calls.iter().filter_map(Weak::upgrade) {
                call.check(now);
            }
        }
    }
}
