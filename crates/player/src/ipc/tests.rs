//! Unit tests for the JSON-to-event translation.

use std::time::Instant;

use serde_json::{Value, json};

use super::{POSITION_INTERVAL, PositionLimiter, State, Translation, translate};
use crate::{Error, PlayerEvent};

fn property(name: &str, data: &Value) -> Value {
    json!({ "event": "property-change", "name": name, "data": data })
}

fn events_of(
    value: &Value,
    state: &mut State,
    positions: &mut PositionLimiter,
) -> Vec<PlayerEvent> {
    match translate(value, state, positions, Instant::now()) {
        Translation::Events(events) => events,
        Translation::Ignore | Translation::Reply { .. } => Vec::new(),
    }
}

#[test]
fn translates_file_loaded_and_properties_in_order() {
    let mut state = State::default();
    let mut positions = PositionLimiter::new(POSITION_INTERVAL);

    assert_eq!(
        events_of(
            &property("path", &json!("/media/ep.mkv")),
            &mut state,
            &mut positions
        ),
        []
    );

    assert_eq!(
        events_of(
            &json!({ "event": "file-loaded" }),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::MediaReady {
            path: Some("/media/ep.mkv".to_owned()),
            duration: None,
        }]
    );

    assert_eq!(
        events_of(&property("pause", &json!(true)), &mut state, &mut positions),
        vec![PlayerEvent::Paused(true)]
    );

    assert_eq!(
        events_of(
            &property(
                "chapter-list",
                &json!([
                    { "title": "OP", "time": 0.0 },
                    { "title": null, "time": 90.0 }
                ])
            ),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::Chapters {
            current: None,
            list: vec![
                crate::Chapter {
                    title: Some("OP".to_owned()),
                    start: 0.0
                },
                crate::Chapter {
                    title: None,
                    start: 90.0
                },
            ],
        }]
    );
}

#[test]
fn eof_is_announced_once_for_property_and_end_file() {
    let mut state = State::default();
    let mut positions = PositionLimiter::new(POSITION_INTERVAL);

    assert_eq!(
        events_of(
            &property("eof-reached", &json!(true)),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::Eof]
    );

    let result = translate(
        &json!({ "event": "end-file", "reason": "eof" }),
        &mut state,
        &mut positions,
        Instant::now(),
    );
    assert!(matches!(result, Translation::Ignore));
}

#[test]
fn a_second_file_can_reach_eof_in_the_same_session() {
    let mut state = State::default();
    let mut positions = PositionLimiter::new(POSITION_INTERVAL);

    assert_eq!(
        events_of(
            &json!({ "event": "file-loaded" }),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::MediaReady {
            path: None,
            duration: None
        }]
    );
    assert_eq!(
        events_of(
            &property("eof-reached", &json!(true)),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::Eof]
    );
    assert_eq!(
        events_of(
            &json!({ "event": "file-loaded" }),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::MediaReady {
            path: None,
            duration: None
        }]
    );
    assert_eq!(
        events_of(
            &property("eof-reached", &json!(true)),
            &mut state,
            &mut positions
        ),
        vec![PlayerEvent::Eof]
    );
}

#[test]
fn reply_success_and_failure_are_typed() {
    let mut state = State::default();
    let mut positions = PositionLimiter::new(POSITION_INTERVAL);

    let ok = translate(
        &json!({ "error": "success", "data": 12.5, "request_id": 9 }),
        &mut state,
        &mut positions,
        Instant::now(),
    );
    assert!(
        matches!(ok, Translation::Reply { id: 9, .. }),
        "expected reply 9"
    );
    let Translation::Reply { result, .. } = ok else {
        return;
    };
    assert_eq!(result.unwrap(), json!(12.5));

    let failed = translate(
        &json!({ "error": "property unavailable", "request_id": 10 }),
        &mut state,
        &mut positions,
        Instant::now(),
    );
    assert!(matches!(failed, Translation::Reply { id: 10, .. }));
    let Translation::Reply { result, .. } = failed else {
        return;
    };
    assert!(matches!(result, Err(Error::Command { .. })));
}

#[test]
fn replies_without_a_caller_are_still_typed() {
    let mut state = State::default();
    let mut positions = PositionLimiter::new(POSITION_INTERVAL);
    let result = translate(
        &json!({ "error": "success", "request_id": 1 }),
        &mut state,
        &mut positions,
        Instant::now(),
    );
    assert!(
        matches!(result, Translation::Reply { .. }),
        "the pump ignores replies that have no waiting caller"
    );
}
