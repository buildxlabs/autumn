//! Settings → Playback: defaults, validation and the database row shape.

use std::path::PathBuf;

use settings::{PlaybackKind, PlaybackSettings, PlayerChoice, Settings, SettingsError};

fn playback() -> PlaybackSettings {
    PlaybackSettings {
        player: PlayerChoice::Installed {
            kind: PlaybackKind::Vlc,
        },
        mpchc_host: "127.0.0.1".to_owned(),
        mpchc_port: 13579,
        watched_threshold: 90,
        auto_sync_progress: false,
        resume: false,
        auto_next: false,
        auto_next_seconds: 3,
        preload_next: false,
        skip_op_ed: false,
        opening_patterns: vec!["Opening".to_owned()],
        ending_patterns: vec!["Ending".to_owned()],
    }
}

#[test]
fn defaults_are_phase_two_policy() {
    let settings = PlaybackSettings::default();

    assert_eq!(settings.player, PlayerChoice::Auto);
    assert_eq!(settings.watched_threshold, 80, "OD-5: 80 percent or EOF");
    assert!(settings.auto_sync_progress, "auto-sync is on by default");
    assert!(settings.resume, "resume where I stopped is on by default");
    assert_eq!(settings.mpchc_host, "127.0.0.1");
    assert_eq!(settings.mpchc_port, 13579);
    assert!(
        settings.preload_next,
        "#208: Preload next episode is on by default"
    );
}

#[test]
fn a_row_saved_before_the_preload_toggle_existed_keeps_it_on() {
    let old: PlaybackSettings = serde_json::from_str(r#"{"auto_next": true}"#).unwrap();

    assert!(old.preload_next);
}

#[test]
fn valid_settings_pass_validation() {
    assert!(playback().validate().is_ok());
    assert!(PlaybackSettings::default().validate().is_ok());
}

#[test]
fn a_threshold_outside_fifty_to_one_hundred_is_rejected() {
    let mut settings = playback();
    settings.watched_threshold = 49;
    assert!(matches!(
        settings.validate(),
        Err(SettingsError::Validation { .. })
    ));

    settings.watched_threshold = 101;
    assert!(matches!(
        settings.validate(),
        Err(SettingsError::Validation { .. })
    ));

    settings.watched_threshold = 50;
    assert!(settings.validate().is_ok(), "inclusive lower bound");
    settings.watched_threshold = 100;
    assert!(settings.validate().is_ok(), "inclusive upper bound");
}

#[test]
fn an_empty_custom_path_is_rejected() {
    let mut settings = playback();
    settings.player = PlayerChoice::Custom {
        kind: PlaybackKind::Mpv,
        path: PathBuf::new(),
    };
    assert!(matches!(
        settings.validate(),
        Err(SettingsError::Validation { .. })
    ));
}

#[test]
fn a_missing_custom_path_is_rejected_and_a_real_one_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let executable = dir.path().join("mpv");
    std::fs::write(&executable, b"not really an executable").unwrap();
    let mut settings = playback();
    settings.player = PlayerChoice::Custom {
        kind: PlaybackKind::Mpv,
        path: dir.path().join("missing-player"),
    };
    assert!(settings.validate().is_err(), "a path that does not exist");

    settings.player = PlayerChoice::Custom {
        kind: PlaybackKind::Mpv,
        path: executable,
    };
    assert!(settings.validate().is_ok());
}

#[test]
fn mpc_hc_fields_are_only_validated_when_mpc_hc_is_selected() {
    let mut settings = playback();
    settings.mpchc_host.clear();
    settings.mpchc_port = 0;
    assert!(
        settings.validate().is_ok(),
        "hidden fields must not block unrelated saves"
    );

    settings.player = PlayerChoice::Installed {
        kind: PlaybackKind::MpcHc,
    };
    assert!(settings.validate().is_err(), "empty host is unusable");
    settings.mpchc_host = "127.0.0.1".to_owned();
    assert!(settings.validate().is_err(), "port zero is unusable");
    settings.mpchc_port = 13579;
    assert!(settings.validate().is_ok());
}

#[test]
fn playback_round_trips_through_settings_rows() {
    let settings = Settings {
        playback: playback(),
        ..Settings::default()
    };
    let rows = settings.to_rows().unwrap();

    let (loaded, errors) = Settings::from_rows(rows);

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(loaded.playback, playback());
    assert_eq!(loaded, settings, "the other tabs are untouched");
}
