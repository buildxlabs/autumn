//! `settings.json` loading and saving, through the crate's public API.

use std::fs;

use settings::{AppearanceSettings, Settings, SettingsError};

#[test]
fn missing_file_yields_defaults_and_writes_them() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");

    let loaded = Settings::load_or_init(&path).unwrap();

    assert_eq!(loaded, Settings::default(), "first run uses the defaults");
    assert!(path.is_file(), "first run writes the defaults to disk");
}

#[test]
fn unknown_fields_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(
        &path,
        r#"{"appearance":{"sidebar_click_pin":true,"retired":1},"from_the_future":{}}"#,
    )
    .unwrap();

    let loaded = Settings::load_or_init(&path).unwrap();

    assert!(
        loaded.appearance.sidebar_click_pin,
        "known fields still load"
    );
}

#[test]
fn missing_fields_take_their_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, r#"{"appearance":{}}"#).unwrap();

    let loaded = Settings::load_or_init(&path).unwrap();

    assert_eq!(
        loaded,
        Settings::default(),
        "every absent field is defaulted"
    );
}

#[test]
fn save_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    let saved = Settings {
        appearance: AppearanceSettings {
            sidebar_click_pin: true,
        },
    };

    saved.save(&path).unwrap();
    let loaded = Settings::load_or_init(&path).unwrap();

    assert_eq!(loaded, saved, "a saved file loads back unchanged");
}

#[test]
fn save_leaves_no_temporary_file_behind() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");

    Settings::default().save(&path).unwrap();
    Settings::default().save(&path).unwrap();

    let names: Vec<_> = fs::read_dir(dir.path())
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(names, ["settings.json"], "only the settings file remains");
}

#[test]
fn corrupt_file_is_an_error_and_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{ not json").unwrap();

    let err = Settings::load_or_init(&path).unwrap_err();

    assert!(matches!(err, SettingsError::Parse { .. }), "got {err:?}");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "{ not json",
        "a corrupt file is never overwritten"
    );
}
