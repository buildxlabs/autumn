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

// --- Profile CRUD (#228): plain functions, not inside `render()`. ---

fn named_with(name: &str, resolutions: &[u32]) -> AutoSelectProfile {
    AutoSelectProfile {
        name: name.to_owned(),
        resolutions: resolutions.to_vec(),
        ..AutoSelectProfile::default()
    }
}

#[test]
fn new_profile_adds_shipped_defaults_under_a_unique_name() {
    let mut settings = AutoSelectSettings::default();

    settings.add_profile("Anime 1080p").unwrap();

    assert_eq!(settings.profiles.len(), 2);
    assert_eq!(
        settings.profiles.last().unwrap(),
        &AutoSelectProfile {
            name: "Anime 1080p".to_owned(),
            ..AutoSelectProfile::default()
        }
    );
    assert!(settings.validate().is_ok());
}

#[test]
fn new_profile_rejects_blank_and_repeated_names() {
    let mut settings = AutoSelectSettings::default();
    settings.add_profile("Two").unwrap();

    assert!(settings.add_profile("  ").is_err());
    assert!(settings.add_profile("two").is_err(), "case-insensitive");
    assert!(settings.add_profile("Default").is_err());
    assert_eq!(settings.profiles.len(), 2);
}

#[test]
fn duplicate_copies_every_field_under_a_fresh_name() {
    let mut settings = AutoSelectSettings::default();
    let mut source = named_with("Custom", &[2160, 1080]);
    source.min_seeders = 5;
    source.excluded_terms = vec!["cam".to_owned()];
    source.batch = Preference::Only;
    settings.profiles.push(source);

    let copy_name = settings.duplicate_profile("Custom").unwrap();

    assert_ne!(copy_name, "Custom");
    let original = settings
        .profiles
        .iter()
        .find(|p| p.name == "Custom")
        .unwrap();
    let copy = settings
        .profiles
        .iter()
        .find(|p| p.name == copy_name)
        .unwrap();
    assert_eq!(original.resolutions, copy.resolutions);
    assert_eq!(original.min_seeders, copy.min_seeders);
    assert_eq!(original.excluded_terms, copy.excluded_terms);
    assert_eq!(original.batch, copy.batch);
    assert!(settings.validate().is_ok());
}

#[test]
fn duplicate_the_shipped_default_keeps_it_and_copies_it() {
    let mut settings = AutoSelectSettings::default();

    let copy_name = settings.duplicate_profile("Default").unwrap();

    assert_ne!(copy_name, "Default");
    assert!(settings.profiles.iter().any(|p| p.name == "Default"));
    assert_eq!(settings.profiles.len(), 2);
    assert!(settings.validate().is_ok());
}

#[test]
fn duplicate_unknown_profile_fails() {
    let mut settings = AutoSelectSettings::default();
    assert!(settings.duplicate_profile("Gone").is_err());
}

#[test]
fn rename_renames_and_updates_every_reference() {
    let mut settings = AutoSelectSettings {
        profiles: vec![named("Default"), named("Small")],
        default_profile: "Small".to_owned(),
        media_profiles: [(1, "Small".to_owned()), (2, "Default".to_owned())].into(),
    };

    settings.rename_profile("Small", "Tiny").unwrap();

    assert!(settings.profiles.iter().any(|p| p.name == "Tiny"));
    assert!(!settings.profiles.iter().any(|p| p.name == "Small"));
    assert_eq!(settings.default_profile, "Tiny");
    assert_eq!(settings.media_profiles.get(&1).unwrap(), "Tiny");
    assert_eq!(settings.effective(Some(1)).name, "Tiny");
    assert!(settings.validate().is_ok());
}

#[test]
fn rename_rejects_blank_repeated_and_unknown_names() {
    let mut settings = AutoSelectSettings::default();
    settings.add_profile("Other").unwrap();

    assert!(settings.rename_profile("Other", " ").is_err());
    assert!(settings.rename_profile("Other", "default").is_err());
    assert!(settings.rename_profile("Gone", "X").is_err());
    // Renaming to your own name is a no-op.
    assert!(settings.rename_profile("Other", "Other").is_ok());
    assert!(settings.validate().is_ok());
}

#[test]
fn the_shipped_default_cannot_be_renamed() {
    let mut settings = AutoSelectSettings::default();
    assert!(settings.rename_profile("Default", "Mine").is_err());
    assert!(settings.validate().is_ok());
}

#[test]
fn delete_removes_profile_and_cleans_references() {
    let mut settings = AutoSelectSettings {
        profiles: vec![named("Default"), named("Small"), named("Dual")],
        default_profile: "Default".to_owned(),
        media_profiles: [(1, "Small".to_owned()), (2, "Dual".to_owned())].into(),
    };

    settings.delete_profile("Small").unwrap();

    assert!(!settings.profiles.iter().any(|p| p.name == "Small"));
    assert!(!settings.media_profiles.values().any(|name| name == "Small"));
    assert!(settings.validate().is_ok());
}

#[test]
fn deleting_the_global_default_falls_back_cleanly() {
    let mut settings = AutoSelectSettings {
        profiles: vec![named("Default"), named("Small")],
        default_profile: "Small".to_owned(),
        media_profiles: [(1, "Small".to_owned())].into(),
    };

    settings.delete_profile("Small").unwrap();

    assert!(settings.validate().is_ok());
    // Global default falls back to the shipped `Default`.
    assert_eq!(settings.default_profile, "Default");
    assert_eq!(settings.effective(None).name, "Default");
    // A per-media override naming the deleted profile is removed; falls back.
    assert_eq!(settings.effective(Some(1)).name, "Default");
}

#[test]
fn the_shipped_default_cannot_be_deleted() {
    let mut settings = AutoSelectSettings::default();
    settings.add_profile("Other").unwrap();
    assert!(settings.delete_profile("Default").is_err());
    assert!(settings.validate().is_ok());
}

#[test]
fn deleting_unknown_profile_fails() {
    let mut settings = AutoSelectSettings::default();
    assert!(settings.delete_profile("Gone").is_err());
}

#[test]
fn set_global_default_requires_an_existing_profile() {
    let mut settings = AutoSelectSettings::default();
    settings.add_profile("Small").unwrap();

    settings.set_global_default("Small").unwrap();
    assert_eq!(settings.default_profile, "Small");
    assert_eq!(settings.effective(None).name, "Small");

    assert!(settings.set_global_default("Gone").is_err());
    assert!(settings.validate().is_ok());
}

#[test]
fn reset_default_restores_the_shipped_profile() {
    let mut settings = AutoSelectSettings::default();
    {
        let default = settings
            .profiles
            .iter_mut()
            .find(|p| p.name == "Default")
            .unwrap();
        default.resolutions = vec![480];
        default.min_seeders = 9;
        default.excluded_terms = vec!["x".to_owned()];
        default.batch = Preference::Only;
        default.require_codec = true;
    }
    // A second profile keeps working and stays put.
    settings.add_profile("Other").unwrap();
    settings.set_global_default("Other").unwrap();

    settings.reset_default_profile();

    let default = settings
        .profiles
        .iter()
        .find(|p| p.name == "Default")
        .unwrap();
    assert_eq!(default, &AutoSelectProfile::default());
    assert!(settings.profiles.iter().any(|p| p.name == "Other"));
    assert_eq!(
        settings.default_profile, "Other",
        "reset doesn't touch the global default"
    );
    assert!(settings.validate().is_ok());
}

#[test]
fn move_item_reorders_without_losing_elements() {
    let mut list = vec![
        "a".to_owned(),
        "b".to_owned(),
        "c".to_owned(),
        "d".to_owned(),
    ];

    settings::move_item(&mut list, 0, 2);
    assert_eq!(list, ["b", "c", "a", "d"]);

    settings::move_item(&mut list, 3, 0);
    assert_eq!(list, ["d", "b", "c", "a"]);

    // Out-of-range and no-op moves leave the list unchanged.
    settings::move_item(&mut list, 9, 0);
    settings::move_item(&mut list, 0, 9);
    settings::move_item(&mut list, 1, 1);
    assert_eq!(list, ["d", "b", "c", "a"]);
}
