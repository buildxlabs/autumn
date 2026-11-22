//! The list entry model and its status.

use domain::{ListStatus, MediaKind};

#[test]
fn every_status_round_trips_through_serde_with_anilist_names() {
    for (status, name) in [
        (ListStatus::Current, "CURRENT"),
        (ListStatus::Planning, "PLANNING"),
        (ListStatus::Completed, "COMPLETED"),
        (ListStatus::Dropped, "DROPPED"),
        (ListStatus::Paused, "PAUSED"),
        (ListStatus::Repeating, "REPEATING"),
    ] {
        let json = serde_json::to_string(&status).unwrap();

        assert_eq!(json, format!("\"{name}\""), "{status:?}");
        assert_eq!(serde_json::from_str::<ListStatus>(&json).unwrap(), status);
    }
}

#[test]
fn unknown_status_is_rejected() {
    assert!(
        serde_json::from_str::<ListStatus>("\"WATCHING\"").is_err(),
        "not AniList"
    );
}

#[test]
fn media_kind_round_trips_through_serde() {
    for (kind, name) in [(MediaKind::Anime, "ANIME"), (MediaKind::Manga, "MANGA")] {
        let json = serde_json::to_string(&kind).unwrap();

        assert_eq!(json, format!("\"{name}\""), "{kind:?}");
        assert_eq!(serde_json::from_str::<MediaKind>(&json).unwrap(), kind);
    }
}
