//! Partial dates, as AniList's `FuzzyDate`.

use domain::{FuzzyDate, FuzzyDateError};

#[test]
fn any_combination_of_known_parts_is_valid() {
    for (year, month, day) in [
        (None, None, None),
        (Some(2024), None, None),
        (Some(2024), Some(4), None),
        (Some(2024), Some(4), Some(30)),
        (None, Some(4), Some(30)),
        (Some(2024), None, Some(12)),
    ] {
        let date = FuzzyDate::new(year, month, day).unwrap();

        assert_eq!((date.year(), date.month(), date.day()), (year, month, day));
    }
}

#[test]
fn the_empty_date_is_unknown() {
    assert!(FuzzyDate::UNKNOWN.is_unknown(), "no parts known");
    assert!(
        !FuzzyDate::new(Some(2024), None, None).unwrap().is_unknown(),
        "year known"
    );
}

#[test]
fn impossible_parts_are_rejected() {
    for (year, month, day, expected) in [
        (None, Some(0), None, FuzzyDateError::Month(0)),
        (None, Some(13), None, FuzzyDateError::Month(13)),
        (None, None, Some(0), FuzzyDateError::Day(0)),
        (None, None, Some(32), FuzzyDateError::Day(32)),
        (Some(2024), Some(4), Some(31), FuzzyDateError::Day(31)),
        (None, Some(2), Some(30), FuzzyDateError::Day(30)),
        (Some(2023), Some(2), Some(29), FuzzyDateError::Day(29)),
    ] {
        assert_eq!(
            FuzzyDate::new(year, month, day),
            Err(expected),
            "{year:?}-{month:?}-{day:?}"
        );
    }
}

#[test]
fn february_29_needs_a_leap_year_or_an_unknown_year() {
    for year in [Some(2024), Some(2000), None] {
        assert!(FuzzyDate::new(year, Some(2), Some(29)).is_ok(), "{year:?}");
    }
    assert!(
        FuzzyDate::new(Some(1900), Some(2), Some(29)).is_err(),
        "1900"
    );
}

#[test]
fn serde_uses_anilist_shape_and_validates() {
    let date = FuzzyDate::new(Some(2024), Some(4), None).unwrap();

    let json = serde_json::to_string(&date).unwrap();
    assert_eq!(
        json, r#"{"year":2024,"month":4,"day":null}"#,
        "AniList shape"
    );
    assert_eq!(
        serde_json::from_str::<FuzzyDate>(&json).unwrap(),
        date,
        "round-trips"
    );
    assert!(
        serde_json::from_str::<FuzzyDate>(r#"{"year":null,"month":13,"day":null}"#).is_err(),
        "month 13 is rejected on load"
    );
}
