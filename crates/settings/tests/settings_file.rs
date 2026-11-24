//! Reading the Phase 0 `settings.json`, through the crate's public API.

use std::fs;

use settings::{AppearanceSettings, LibrarySettings, Settings, SettingsError};

#[test]
fn missing_file_reads_as_none() {
    let dir = tempfile::tempdir().unwrap();

    let read = Settings::read_file(&dir.path().join("settings.json")).unwrap();

    assert_eq!(read, None, "no file, nothing to import");
}

#[test]
fn a_valid_file_reads_its_values() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, r#"{"appearance":{"sidebar_click_pin":true}}"#).unwrap();

    let read = Settings::read_file(&path).unwrap();

    let expected = Settings {
        appearance: AppearanceSettings {
            sidebar_click_pin: true,
            ..AppearanceSettings::default()
        },
        library: LibrarySettings::default(),
        playback: settings::PlaybackSettings::default(),
    };
    assert_eq!(read, Some(expected));
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

    let read = Settings::read_file(&path).unwrap().unwrap();

    assert!(read.appearance.sidebar_click_pin, "known fields still load");
}

#[test]
fn missing_fields_take_their_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, r#"{"appearance":{}}"#).unwrap();

    let read = Settings::read_file(&path).unwrap();

    assert_eq!(
        read,
        Some(Settings::default()),
        "every absent field is defaulted"
    );
}

#[test]
fn corrupt_file_is_a_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("settings.json");
    fs::write(&path, "{ not json").unwrap();

    let err = Settings::read_file(&path).unwrap_err();

    assert!(matches!(err, SettingsError::Parse { .. }), "got {err:?}");
}
