//! First-run wizard settings (#138, ROADMAP Phase 2).

use std::path::PathBuf;

use settings::{LibrarySettings, PlaybackKind, PlaybackSettings, PlayerChoice, Settings};

#[test]
fn a_fresh_install_needs_onboarding() {
    let settings = Settings::default();

    assert!(
        !settings.general.onboarding_completed,
        "a new install starts with the wizard pending"
    );
    let rows = settings.to_rows().expect("settings encode as rows");
    let general = rows
        .iter()
        .find(|(tab, _)| tab == "general")
        .map(|(_, json)| json.as_str());
    assert_eq!(
        general,
        Some(r#"{"onboarding_completed":false}"#),
        "the flag has its own row, false by default"
    );
}

#[test]
fn skip_completes_onboarding_without_changing_the_other_tabs() {
    let mut settings = Settings::default();
    settings.complete_onboarding();

    assert!(
        settings.general.onboarding_completed,
        "Skip completes the wizard"
    );
    assert_eq!(settings.library, LibrarySettings::default());
    assert_eq!(settings.playback, PlaybackSettings::default());
    let (loaded, errors) = Settings::from_rows(settings.to_rows().expect("rows encode"));
    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        loaded.general.onboarding_completed,
        "the completion survives a settings reload"
    );
}

#[test]
fn an_existing_install_is_prefilled_and_still_sees_the_wizard_once() {
    let mut existing = Settings::default();
    existing.library.folders = vec![PathBuf::from("/shows")];
    existing.playback.player = PlayerChoice::Installed {
        kind: PlaybackKind::Mpv,
    };
    // Phase 1 wrote three rows; there was no General tab yet.
    let rows = existing
        .to_rows()
        .expect("rows encode")
        .into_iter()
        .filter(|(tab, _)| tab != "general")
        .collect::<Vec<_>>();

    let (loaded, errors) = Settings::from_rows(rows);

    assert!(errors.is_empty(), "{errors:?}");
    assert!(
        !loaded.general.onboarding_completed,
        "a missing flag means the wizard is still pending"
    );
    assert_eq!(
        loaded.library.folders,
        [PathBuf::from("/shows")],
        "the chosen library folder is prefilled"
    );
    assert_eq!(
        loaded.playback.player,
        PlayerChoice::Installed {
            kind: PlaybackKind::Mpv
        },
        "the chosen player is prefilled"
    );

    let mut reopened = loaded;
    reopened.reopen_onboarding();
    assert!(!reopened.general.onboarding_completed);
    reopened.complete_onboarding();
    assert!(reopened.general.onboarding_completed);
}
