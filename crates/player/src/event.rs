//! Typed events produced by the player IPC pump.

use std::time::{Duration, Instant};

use serde::Deserialize;

/// A chapter marker, as mpv reports it in `chapter-list`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Chapter {
    /// The chapter title, if mpv supplied one.
    pub title: Option<String>,
    /// Where the chapter starts, in seconds.
    #[serde(rename = "time")]
    pub start: f64,
}

/// Something the player did that the engine may react to.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum PlayerEvent {
    /// A file is loaded and ready for resume and chapter setup.
    MediaReady {
        /// The player's `path`, when it reported one.
        path: Option<String>,
        /// The file's duration in seconds, when known.
        duration: Option<f64>,
    },
    /// Playback position, coalesced to at most four times a second.
    Position {
        /// Seconds from the start of the file.
        seconds: f64,
    },
    /// Pause state changed.
    Paused(bool),
    /// The player reached the end of the file.
    Eof,
    /// The player process or its IPC connection ended.
    Exited,
    /// The chapter list or the current chapter changed.
    Chapters {
        /// The zero-based current chapter, if one is selected.
        current: Option<usize>,
        /// The full chapter list.
        list: Vec<Chapter>,
    },
}

/// Coalesces a fast stream of positions into at most one per interval.
///
/// The clock is passed in so the policy is testable without sleeping. The
/// live pump calls [`take`](Self::take) on a 250 ms tick and emits the result.
#[derive(Debug)]
pub(crate) struct PositionLimiter {
    interval: Duration,
    pending: Option<f64>,
    last: Option<Instant>,
}

impl PositionLimiter {
    /// A limiter that emits at most one value per `interval`.
    pub(crate) const fn new(interval: Duration) -> Self {
        Self {
            interval,
            pending: None,
            last: None,
        }
    }

    /// Records the newest position, replacing any older one waiting to be sent.
    pub(crate) fn push(&mut self, seconds: f64) {
        self.pending = Some(seconds);
    }

    /// Returns the pending position once the interval has elapsed.
    pub(crate) fn take(&mut self, now: Instant) -> Option<f64> {
        let ready = self
            .last
            .is_none_or(|last| now.duration_since(last) >= self.interval);
        if !ready {
            return None;
        }
        let value = self.pending.take();
        if value.is_some() {
            self.last = Some(now);
        }
        value
    }

    /// Drops any un-emitted position, used when the session ends.
    pub(crate) fn clear(&mut self) {
        self.pending = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_the_newest_of_a_burst_once_per_interval() {
        let start = Instant::now();
        let mut limiter = PositionLimiter::new(Duration::from_millis(250));

        limiter.push(1.0);
        limiter.push(2.0);
        limiter.push(3.0);
        assert_eq!(limiter.take(start), Some(3.0));
        assert_eq!(limiter.take(start + Duration::from_millis(100)), None);

        limiter.push(4.0);
        limiter.push(5.0);
        assert_eq!(limiter.take(start + Duration::from_millis(100)), None);
        assert_eq!(limiter.take(start + Duration::from_millis(250)), Some(5.0));
    }

    #[test]
    fn a_new_position_after_the_interval_is_emitted() {
        let start = Instant::now();
        let mut limiter = PositionLimiter::new(Duration::from_millis(250));

        assert_eq!(limiter.take(start), None);
        limiter.push(7.5);
        assert_eq!(limiter.take(start + Duration::from_millis(250)), Some(7.5));
    }

    #[test]
    fn clear_discards_a_position_that_was_never_sent() {
        let start = Instant::now();
        let mut limiter = PositionLimiter::new(Duration::from_millis(250));

        limiter.push(9.0);
        limiter.clear();
        assert_eq!(limiter.take(start + Duration::from_secs(1)), None);
    }
}
