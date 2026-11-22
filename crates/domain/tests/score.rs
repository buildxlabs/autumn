//! Raw 0–100 scores and their `POINT_10_DECIMAL` display form.

use domain::{Score, ScoreError};

#[test]
fn raw_scores_convert_to_point_10_decimal() {
    for (raw, shown) in [(1, 0.1), (10, 1.0), (85, 8.5), (100, 10.0)] {
        let score = Score::new(raw).unwrap();

        assert_eq!(score.to_point_10_decimal(), Some(shown), "raw {raw}");
    }
}

#[test]
fn point_10_decimal_converts_back_to_raw() {
    for (shown, raw) in [(0.1, 1), (1.0, 10), (8.5, 85), (7.3, 73), (10.0, 100)] {
        let score = Score::from_point_10_decimal(shown).unwrap();

        assert_eq!(score.raw(), raw, "shown {shown}");
    }
}

#[test]
fn zero_means_unscored_both_ways() {
    assert_eq!(Score::UNSCORED.raw(), 0, "unscored is stored as 0");
    assert!(!Score::UNSCORED.is_scored(), "0 is not a score");
    assert_eq!(
        Score::UNSCORED.to_point_10_decimal(),
        None,
        "nothing to show"
    );
    assert_eq!(
        Score::from_point_10_decimal(0.0).unwrap(),
        Score::UNSCORED,
        "0.0 entered clears the score"
    );
    assert_eq!(Score::default(), Score::UNSCORED, "entries start unscored");
}

#[test]
fn out_of_range_scores_are_rejected() {
    assert!(
        matches!(Score::new(101), Err(ScoreError::OutOfRange)),
        "raw > 100"
    );
    for shown in [-0.1, 10.1, f32::NAN, f32::INFINITY] {
        assert!(
            matches!(
                Score::from_point_10_decimal(shown),
                Err(ScoreError::OutOfRange)
            ),
            "shown {shown}"
        );
    }
}

#[test]
fn score_serializes_as_the_raw_number_and_validates_on_load() {
    let score = Score::new(85).unwrap();

    assert_eq!(serde_json::to_string(&score).unwrap(), "85", "raw number");
    assert_eq!(
        serde_json::from_str::<Score>("85").unwrap(),
        score,
        "round-trips"
    );
    assert!(
        serde_json::from_str::<Score>("101").is_err(),
        "101 is rejected"
    );
}
