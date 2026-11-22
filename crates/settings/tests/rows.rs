//! Settings as one JSON row per tab, the shape `db` stores.

use settings::{AppearanceSettings, Settings};

fn pinned() -> Settings {
    Settings {
        appearance: AppearanceSettings {
            sidebar_click_pin: true,
        },
    }
}

#[test]
fn rows_round_trip() {
    let rows = pinned().to_rows().unwrap();

    let (settings, errors) = Settings::from_rows(rows);

    assert_eq!(settings, pinned());
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn there_is_one_row_per_tab() {
    let rows = Settings::default().to_rows().unwrap();

    let tabs: Vec<&str> = rows.iter().map(|(tab, _)| tab.as_str()).collect();
    assert_eq!(tabs, ["appearance"]);
}

#[test]
fn no_rows_means_defaults() {
    let (settings, errors) = Settings::from_rows([]);

    assert_eq!(settings, Settings::default(), "fresh install");
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn unknown_tabs_are_ignored() {
    let mut rows = pinned().to_rows().unwrap();
    rows.push(("from_the_future".to_owned(), "{}".to_owned()));

    let (settings, errors) = Settings::from_rows(rows);

    assert_eq!(settings, pinned());
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn a_bad_row_takes_its_defaults_and_is_reported() {
    let rows = [(
        "appearance".to_owned(),
        r#"{"sidebar_click_pin":"yes"}"#.to_owned(),
    )];

    let (settings, errors) = Settings::from_rows(rows);

    assert_eq!(
        settings,
        Settings::default(),
        "the bad tab takes its defaults"
    );
    assert_eq!(errors.len(), 1, "the bad row is reported: {errors:?}");
}
