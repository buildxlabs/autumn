//! Auto-select profiles: the shipped `Default`, the global default and the
//! per-media override (#181 Q8, Q10).

use settings::{AutoSelectProfile, AutoSelectSettings, Preference, Settings, SettingsError};

fn named(name: &str) -> AutoSelectProfile {
    AutoSelectProfile {
        name: name.to_owned(),
        ..AutoSelectProfile::default()
    }
}

#[test]
fn the_shipped_default_profile_has_the_q8_values() {
    let profile = AutoSelectProfile::default();

    assert_eq!(profile.name, "Default");
    assert_eq!(profile.resolutions, [1080, 720, 480]);
    assert_eq!(profile.release_groups, Vec::<String>::new());
    assert!(
        profile.codecs.is_empty(),
        "codec is not a filter by default"
    );
    assert_eq!(profile.min_seeders, 0);
    assert_eq!((profile.min_size, profile.max_size), (None, None));
    assert_eq!(profile.excluded_terms, Vec::<String>::new());
    assert_eq!(profile.preferred_languages, Vec::<String>::new());
    assert_eq!(profile.sources, []);
    assert_eq!(profile.multiple_audio, Preference::Neutral);
    assert_eq!(profile.batch, Preference::Neutral);
    assert_eq!(profile.multiple_subs, Preference::Neutral);
}

#[test]
fn a_new_install_has_only_the_default_profile_active_everywhere() {
    let settings = AutoSelectSettings::default();

    assert_eq!(settings.profiles, [AutoSelectProfile::default()]);
    assert_eq!(settings.default_profile, "Default");
    assert!(settings.media_profiles.is_empty());
    assert_eq!(settings.effective(Some(7)), AutoSelectProfile::default());
    assert!(settings.validate().is_ok());
}

#[test]
fn a_media_override_wins_over_the_global_default() {
    let settings = AutoSelectSettings {
        profiles: vec![named("Default"), named("Small"), named("Dual")],
        default_profile: "Small".to_owned(),
        media_profiles: [(42, "Dual".to_owned())].into(),
    };

    assert_eq!(settings.effective(Some(42)).name, "Dual");
    assert_eq!(settings.effective(Some(7)).name, "Small");
    assert_eq!(settings.effective(None).name, "Small");
}

#[test]
fn a_dangling_name_falls_back_to_the_shipped_default() {
    let settings = AutoSelectSettings {
        profiles: vec![named("Small")],
        default_profile: "Gone".to_owned(),
        media_profiles: [(42, "Also gone".to_owned())].into(),
    };

    assert_eq!(settings.effective(Some(42)), AutoSelectProfile::default());
}

#[test]
fn validation_rejects_unusable_profiles() {
    let field = |settings: &AutoSelectSettings| match settings.validate() {
        Err(SettingsError::Validation { field, .. }) => Some(field),
        _ => None,
    };

    let mut empty_name = AutoSelectSettings::default();
    empty_name.profiles.push(named(" "));
    assert_eq!(field(&empty_name), Some("auto_select.profiles.name"));

    let mut duplicate = AutoSelectSettings::default();
    duplicate.profiles.push(named("default"));
    assert_eq!(field(&duplicate), Some("auto_select.profiles.name"));

    let sizes = AutoSelectSettings {
        profiles: vec![AutoSelectProfile {
            min_size: Some(2),
            max_size: Some(1),
            ..AutoSelectProfile::default()
        }],
        ..AutoSelectSettings::default()
    };
    assert_eq!(field(&sizes), Some("auto_select.profiles.max_size"));

    let missing_default = AutoSelectSettings {
        default_profile: "Gone".to_owned(),
        ..AutoSelectSettings::default()
    };
    assert_eq!(field(&missing_default), Some("auto_select.default_profile"));

    let missing_override = AutoSelectSettings {
        media_profiles: [(1, "Gone".to_owned())].into(),
        ..AutoSelectSettings::default()
    };
    assert_eq!(field(&missing_override), Some("auto_select.media_profiles"));
}

#[test]
fn profiles_round_trip_through_their_settings_row() {
    let mut settings = Settings::default();
    settings.auto_select.profiles.push(AutoSelectProfile {
        name: "Dual".to_owned(),
        multiple_audio: Preference::Only,
        max_size: Some(2_000_000_000),
        ..AutoSelectProfile::default()
    });
    settings
        .auto_select
        .media_profiles
        .insert(42, "Dual".to_owned());

    let rows = settings.to_rows().unwrap();
    assert!(rows.iter().any(|(tab, _)| tab == "auto_select"));
    let (loaded, errors) = Settings::from_rows(rows);

    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(loaded.auto_select, settings.auto_select);
}
