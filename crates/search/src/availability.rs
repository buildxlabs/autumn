//! Episode availability badges (#181 Q12–Q13).
//!
//! A badge answers one question: does a release matching this episode exist on
//! the indexers right now? Matching is profile-independent and never fetches
//! a torrent's file list (ADR 0007). Only episodes aired within the last 24 h
//! are checked; `waiting` rechecks every 5 min until that window closes and
//! `available` stops. Every result is cached per (media, episode, indexer),
//! with at most [`MAX_IN_FLIGHT`] checks running at once. The caller persists
//! those results through [`AvailabilityTracker::cached`] and restores them
//! with [`AvailabilityTracker::restore`] (#251).

use std::{
    collections::{HashMap, HashSet, VecDeque},
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, SystemTime},
};

use time::{Date, Month, OffsetDateTime, Time};

use crate::Indexer;

/// How long after airing an episode is still worth checking.
pub const FRESH_WINDOW: Duration = Duration::from_hours(24);
/// How long a `waiting` episode rests before its next check.
pub const RECHECK_INTERVAL: Duration = Duration::from_mins(5);
/// The most availability searches allowed at the same time.
pub const MAX_IN_FLIGHT: usize = 4;

/// What an episode's availability pill says (#181 Q12).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the five states are fixed by #181 Q12"
)]
pub enum Availability {
    /// A matching release is on at least one indexer.
    Available,
    /// A check is running; callers render a pill but not a conclusion.
    Checking,
    /// Checked recently, with no matching release yet.
    Waiting,
    /// No air date, an indexer error, or no usable indexer. No pill.
    Unknown,
    /// Nothing to check (unaired, or older than [`FRESH_WINDOW`]). No pill.
    #[default]
    None,
}

impl Availability {
    /// The label for the three pill states; `None` for `unknown` and `none`.
    #[must_use]
    pub const fn label(self) -> Option<&'static str> {
        match self {
            Self::Available => Some("available"),
            Self::Checking => Some("checking"),
            Self::Waiting => Some("waiting"),
            Self::Unknown | Self::None => None,
        }
    }
}

/// The wall clock used for airing rules. Separate from the searcher's
/// [`crate::search::Clock`], which measures only cache TTLs.
pub trait WallClock: Send + Sync {
    /// The current time.
    fn now(&self) -> SystemTime;
}

/// The production wall clock.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemWallClock;

impl WallClock for SystemWallClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

/// A wall clock advanced explicitly by tests.
#[derive(Debug, Clone)]
pub struct FakeWallClock {
    now: Arc<Mutex<SystemTime>>,
}

impl FakeWallClock {
    /// A clock starting at `now`.
    #[must_use]
    pub fn new(now: SystemTime) -> Self {
        Self {
            now: Arc::new(Mutex::new(now)),
        }
    }

    /// Move the clock forward.
    pub fn advance(&self, duration: Duration) {
        let mut now = self.now.lock().unwrap_or_else(PoisonError::into_inner);
        *now += duration;
    }
}

impl WallClock for FakeWallClock {
    fn now(&self) -> SystemTime {
        *self.now.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[derive(Debug, Clone, Copy)]
struct Checked {
    found: bool,
    at: SystemTime,
}

/// Schedules checks and holds their results, per (media, episode, indexer).
///
/// The caller owns the network calls: [`Self::pump`] grants up to
/// [`MAX_IN_FLIGHT`] keys, then [`Self::complete`] or [`Self::fail`] records
/// the outcome and frees the slot.
#[derive(Debug)]
pub struct AvailabilityTracker<C = SystemWallClock> {
    clock: C,
    aired: HashMap<(i64, u32), SystemTime>,
    results: HashMap<(i64, u32, Indexer), Checked>,
    failed: HashSet<(i64, u32)>,
    in_flight: HashSet<(i64, u32)>,
    pending: VecDeque<(i64, u32)>,
}

impl Default for AvailabilityTracker<SystemWallClock> {
    fn default() -> Self {
        Self::with_clock(SystemWallClock)
    }
}

impl AvailabilityTracker<SystemWallClock> {
    /// A tracker on the wall clock, with no episodes or results yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl<C: WallClock> AvailabilityTracker<C> {
    /// A tracker on an injected clock.
    #[must_use]
    pub fn with_clock(clock: C) -> Self {
        Self {
            clock,
            aired: HashMap::new(),
            results: HashMap::new(),
            failed: HashSet::new(),
            in_flight: HashSet::new(),
            pending: VecDeque::new(),
        }
    }

    /// Register an episode's air date and queue it for a check. Dates outside
    /// the 24 h window, including future ones, are never checked.
    pub fn track(&mut self, media: i64, episode: u32, aired_at: SystemTime) {
        let key = (media, episode);
        self.aired.insert(key, aired_at);
        self.enqueue(media, episode);
    }

    /// Queue an episode again, for the next 5-minute recheck.
    pub fn enqueue(&mut self, media: i64, episode: u32) {
        let key = (media, episode);
        if self.needs_check(key) && !self.pending.contains(&key) {
            self.pending.push_back(key);
        }
    }

    /// Grant checks to queued episodes, up to [`MAX_IN_FLIGHT`].
    #[must_use]
    pub fn pump(&mut self) -> Vec<(i64, u32)> {
        let mut started = Vec::new();
        let mut deferred = VecDeque::new();
        while started.len() < MAX_IN_FLIGHT {
            let Some(key) = self.pending.pop_front() else {
                break;
            };
            if self.begin(key.0, key.1) {
                started.push(key);
            } else if self.needs_check(key) {
                deferred.push_back(key);
            }
        }
        for key in deferred.into_iter().rev() {
            self.pending.push_front(key);
        }
        started
    }

    /// Try to start one check. Fails at the concurrency cap or when the
    /// episode has nothing left to learn.
    #[must_use]
    pub fn begin(&mut self, media: i64, episode: u32) -> bool {
        let key = (media, episode);
        if self.in_flight.len() >= MAX_IN_FLIGHT || self.in_flight.contains(&key) {
            return false;
        }
        if !self.needs_check(key) {
            return false;
        }
        self.in_flight.insert(key);
        true
    }

    /// Record the indexers on which a matching release was found. Indexers
    /// not in `found` are cached as empty for this episode.
    pub fn complete(&mut self, media: i64, episode: u32, found: &[Indexer]) {
        let key = (media, episode);
        let at = self.clock.now();
        for indexer in [Indexer::Nyaa, Indexer::AnimeTosho] {
            self.results.insert(
                (media, episode, indexer),
                Checked {
                    found: found.contains(&indexer),
                    at,
                },
            );
        }
        self.in_flight.remove(&key);
    }

    /// Restore results persisted by an earlier session (#251). A newer result
    /// already in the tracker wins, so a live check is never overwritten by a
    /// stale row.
    pub fn restore(
        &mut self,
        media: i64,
        episode: u32,
        checks: impl IntoIterator<Item = (Indexer, bool, SystemTime)>,
    ) {
        for (indexer, found, at) in checks {
            let key = (media, episode, indexer);
            if self.results.get(&key).is_none_or(|checked| checked.at < at) {
                self.results.insert(key, Checked { found, at });
            }
        }
    }

    /// The cached per-indexer answers for `episode`, for persistence.
    #[must_use]
    pub fn cached(&self, media: i64, episode: u32) -> Vec<(Indexer, bool, SystemTime)> {
        [Indexer::Nyaa, Indexer::AnimeTosho]
            .into_iter()
            .filter_map(|indexer| {
                self.results
                    .get(&(media, episode, indexer))
                    .map(|checked| (indexer, checked.found, checked.at))
            })
            .collect()
    }

    /// Record a failed check: the badge becomes `unknown` and stops.
    pub fn fail(&mut self, media: i64, episode: u32) {
        let key = (media, episode);
        self.failed.insert(key);
        self.in_flight.remove(&key);
    }

    /// The badge to render for `episode`.
    #[must_use]
    pub fn state(&self, media: i64, episode: u32) -> Availability {
        self.state_at((media, episode), self.clock.now())
    }

    /// How long to wait before rechecking, or `None` when the episode has
    /// finished (available, failed, or outside the 24 h window).
    #[must_use]
    pub fn recheck_delay(&self, media: i64, episode: u32) -> Option<Duration> {
        let key = (media, episode);
        let now = self.clock.now();
        if self.state_at(key, now) != Availability::Waiting {
            return None;
        }
        let age = now.duration_since(*self.aired.get(&key)?).ok()?;
        Some(RECHECK_INTERVAL.min(FRESH_WINDOW.checked_sub(age)?))
    }

    fn state_at(&self, key: (i64, u32), now: SystemTime) -> Availability {
        let Some(aired_at) = self.aired.get(&key) else {
            return Availability::None;
        };
        let Ok(age) = now.duration_since(*aired_at) else {
            return Availability::None;
        };
        if age >= FRESH_WINDOW {
            return Availability::None;
        }
        if self.in_flight.contains(&key) {
            return Availability::Checking;
        }
        if self.failed.contains(&key) {
            return Availability::Unknown;
        }
        let all = [Indexer::Nyaa, Indexer::AnimeTosho];
        if !all
            .iter()
            .any(|indexer| self.results.contains_key(&(key.0, key.1, *indexer)))
        {
            return Availability::Waiting;
        }
        if all.iter().any(|indexer| {
            self.results
                .get(&(key.0, key.1, *indexer))
                .is_some_and(|checked| checked.found)
        }) {
            Availability::Available
        } else {
            Availability::Waiting
        }
    }

    fn needs_check(&self, key: (i64, u32)) -> bool {
        let Some(aired_at) = self.aired.get(&key) else {
            return false;
        };
        let now = self.clock.now();
        let Ok(age) = now.duration_since(*aired_at) else {
            return false;
        };
        if age >= FRESH_WINDOW || self.failed.contains(&key) {
            return false;
        }
        if self.state_at(key, now) == Availability::Available {
            return false;
        }
        let last = [Indexer::Nyaa, Indexer::AnimeTosho]
            .iter()
            .filter_map(|indexer| self.results.get(&(key.0, key.1, *indexer)))
            .map(|checked| checked.at)
            .min();
        last.is_none_or(|at| {
            now.duration_since(at)
                .is_ok_and(|since| since >= RECHECK_INTERVAL)
        })
    }
}

/// Parse an `AniZip` air date (`YYYY-MM-DD`, optionally with a time) as UTC.
/// Unknown or malformed values give no badge.
#[must_use]
pub fn parse_air_date(raw: &str) -> Option<SystemTime> {
    let (date_part, time_part) = match raw.split_once(['T', ' ']) {
        Some((date, time)) => (date, Some(time)),
        None => (raw, None),
    };
    let mut date_parts = date_part.split('-');
    let year = date_parts.next()?.parse::<i32>().ok()?;
    let month = date_parts.next()?.parse::<u8>().ok()?;
    let day = date_parts.next()?.parse::<u8>().ok()?;
    let date = Date::from_calendar_date(year, Month::try_from(month).ok()?, day).ok()?;
    let time = match time_part {
        Some(part) => {
            let mut time_parts = part.split(':');
            let hour = time_parts.next()?.parse::<u8>().ok()?;
            let minute = time_parts.next().unwrap_or("0").parse::<u8>().ok()?;
            Time::from_hms(hour, minute, 0).ok()?
        }
        None => Time::MIDNIGHT,
    };
    Some(OffsetDateTime::new_utc(date, time).into())
}
