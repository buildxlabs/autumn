//! Scheduling, recheck and stop rules for availability badges (#226, #181 Q13).

use std::time::Duration;

use search::{Availability, AvailabilityTracker, FakeWallClock, Indexer, parse_air_date};
use time::macros::datetime;

#[test]
fn only_episodes_aired_within_the_last_day_are_checked() {
    let clock = FakeWallClock::new(datetime!(2026-11-28 12:00 UTC).into());
    let mut tracker = AvailabilityTracker::with_clock(clock);
    tracker.track(7, 1, datetime!(2026-11-28 11:00 UTC).into());
    tracker.track(7, 2, datetime!(2026-11-27 10:00 UTC).into());
    tracker.track(7, 3, datetime!(2026-11-29 00:00 UTC).into());

    assert_eq!(
        tracker.pump(),
        vec![(7, 1)],
        "only the fresh episode is due"
    );
    assert_eq!(tracker.state(7, 2), Availability::None, "too old");
    assert_eq!(tracker.state(7, 3), Availability::None, "not aired yet");
}

#[test]
fn a_waiting_episode_is_checked_every_five_minutes() {
    let clock = FakeWallClock::new(datetime!(2026-11-28 12:00 UTC).into());
    let mut tracker = AvailabilityTracker::with_clock(clock.clone());
    tracker.track(7, 1, datetime!(2026-11-28 11:30 UTC).into());
    assert_eq!(tracker.pump(), vec![(7, 1)]);
    assert_eq!(tracker.state(7, 1), Availability::Checking);
    tracker.complete(7, 1, &[]);
    assert_eq!(tracker.state(7, 1), Availability::Waiting);
    assert_eq!(tracker.recheck_delay(7, 1), Some(Duration::from_mins(5)));

    clock.advance(Duration::from_secs(299));
    tracker.enqueue(7, 1);
    assert!(
        tracker.pump().is_empty(),
        "four minutes fifty-nine is too soon"
    );

    clock.advance(Duration::from_secs(1));
    tracker.enqueue(7, 1);
    assert_eq!(tracker.pump(), vec![(7, 1)], "five minutes is due");
}

#[test]
fn an_available_episode_stops_rechecking() {
    let clock = FakeWallClock::new(datetime!(2026-11-28 12:00 UTC).into());
    let mut tracker = AvailabilityTracker::with_clock(clock.clone());
    tracker.track(7, 1, datetime!(2026-11-28 11:30 UTC).into());
    assert_eq!(tracker.pump(), vec![(7, 1)]);
    tracker.complete(7, 1, &[Indexer::Nyaa]);

    assert_eq!(tracker.state(7, 1), Availability::Available);
    assert_eq!(tracker.recheck_delay(7, 1), None);
    clock.advance(Duration::from_mins(10));
    tracker.enqueue(7, 1);
    assert!(
        tracker.pump().is_empty(),
        "available results are not rechecked"
    );
}

#[test]
fn rechecking_stops_at_aired_at_plus_one_day() {
    let clock = FakeWallClock::new(datetime!(2026-11-28 12:00 UTC).into());
    let mut tracker = AvailabilityTracker::with_clock(clock.clone());
    tracker.track(7, 1, datetime!(2026-11-28 11:00 UTC).into());
    assert_eq!(tracker.pump(), vec![(7, 1)]);
    tracker.complete(7, 1, &[]);

    clock.advance(Duration::from_hours(13));
    tracker.enqueue(7, 1);
    assert_eq!(tracker.pump(), vec![(7, 1)], "still inside the window");
    tracker.complete(7, 1, &[]);

    clock.advance(Duration::from_hours(11));
    tracker.enqueue(7, 1);
    assert!(tracker.pump().is_empty(), "the 24 h window has closed");
    assert_eq!(tracker.state(7, 1), Availability::None);
}

#[test]
fn at_most_four_checks_are_in_flight() {
    let clock = FakeWallClock::new(datetime!(2026-11-28 12:00 UTC).into());
    let mut tracker = AvailabilityTracker::with_clock(clock);
    for episode in 1..=6 {
        tracker.track(7, episode, datetime!(2026-11-28 11:00 UTC).into());
    }

    assert_eq!(tracker.pump().len(), 4);
    assert!(!tracker.begin(7, 5), "a fifth check is refused");
    tracker.complete(7, 1, &[]);
    assert_eq!(tracker.pump(), vec![(7, 5)]);
    tracker.complete(7, 2, &[]);
    assert_eq!(tracker.pump(), vec![(7, 6)]);
}

#[test]
fn an_indexer_error_leaves_an_unknown_badge_and_stops() {
    let clock = FakeWallClock::new(datetime!(2026-11-28 12:00 UTC).into());
    let mut tracker = AvailabilityTracker::with_clock(clock.clone());
    tracker.track(7, 1, datetime!(2026-11-28 11:00 UTC).into());
    assert_eq!(tracker.pump(), vec![(7, 1)]);
    tracker.fail(7, 1);

    assert_eq!(tracker.state(7, 1), Availability::Unknown);
    assert_eq!(tracker.recheck_delay(7, 1), None);
    clock.advance(Duration::from_mins(10));
    tracker.enqueue(7, 1);
    assert!(tracker.pump().is_empty(), "a failed check is not retried");
}

#[test]
fn only_available_checking_and_waiting_have_pill_labels() {
    assert_eq!(Availability::Available.label(), Some("available"));
    assert_eq!(Availability::Checking.label(), Some("checking"));
    assert_eq!(Availability::Waiting.label(), Some("waiting"));
    assert_eq!(Availability::Unknown.label(), None);
    assert_eq!(Availability::None.label(), None);
}

#[test]
fn air_dates_parse_as_utc() {
    assert_eq!(
        parse_air_date("2026-11-28"),
        Some(datetime!(2026-11-28 00:00 UTC).into())
    );
    assert_eq!(
        parse_air_date("2026-11-28T20:30:00+00:00"),
        Some(datetime!(2026-11-28 20:30 UTC).into())
    );
    assert_eq!(parse_air_date("not a date"), None);
}
