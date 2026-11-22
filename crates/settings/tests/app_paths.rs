//! App directory creation, through the crate's public API.

use settings::AppPaths;

#[test]
fn create_dirs_creates_all_three_and_is_idempotent() {
    let root = tempfile::tempdir().unwrap();
    let paths = AppPaths::new(
        root.path().join("config"),
        root.path().join("data"),
        root.path().join("cache/nested"),
    );

    paths.create_dirs().unwrap();
    paths.create_dirs().unwrap();

    assert!(paths.config_dir().is_dir(), "config dir exists");
    assert!(paths.data_dir().is_dir(), "data dir exists");
    assert!(paths.cache_dir().is_dir(), "cache dir exists");
}

#[test]
fn files_live_where_architecture_section_8_puts_them() {
    let paths = AppPaths::new("/c".into(), "/d".into(), "/k".into());

    assert_eq!(
        paths.settings_file(),
        std::path::Path::new("/c/settings.json")
    );
    assert_eq!(paths.log_dir(), std::path::Path::new("/d/logs"));
}

#[test]
fn resolve_uses_the_autumn_project_dirs() {
    let paths = AppPaths::resolve().unwrap();

    let config = paths.config_dir().to_string_lossy().to_lowercase();
    assert!(
        config.contains("autumn"),
        "config dir {config} names the app"
    );
}
