//! Settings → Storage: the torrent download folder (ADR-0006, #180 Q1/Q4).

use std::path::PathBuf;

use settings::{AppPaths, Settings, SettingsError, TorrentSettings};

fn paths() -> AppPaths {
    AppPaths::new(
        PathBuf::from("/app/config"),
        PathBuf::from("/app/data"),
        PathBuf::from("/app/cache"),
    )
}

fn with_folders(folder: Option<PathBuf>, library: Vec<PathBuf>) -> Settings {
    Settings {
        library: settings::LibrarySettings {
            folders: library,
            ..settings::LibrarySettings::default()
        },
        torrent: TorrentSettings {
            download_folder: folder,
            ..TorrentSettings::default()
        },
        ..Settings::default()
    }
}

fn is_overlap_error(result: &Result<(), SettingsError>) -> bool {
    matches!(
        result,
        Err(SettingsError::Validation {
            field: "torrent.download_folder",
            ..
        })
    )
}

#[test]
fn the_default_folder_is_the_os_cache_dir_and_upload_stays_on() {
    let paths = paths();
    let settings = Settings::default();

    assert_eq!(settings.torrent_folder(&paths), paths.torrent_dir());
    assert!(!settings.torrent.disable_upload);
    assert!(
        settings
            .validate_torrent_folder(&paths.torrent_dir())
            .is_ok(),
        "an empty library must accept the default"
    );
}

#[test]
fn a_folder_equal_to_a_library_folder_is_rejected() {
    let folder = PathBuf::from("/media/anime");
    let settings = with_folders(
        Some(folder),
        vec![PathBuf::from("/media/anime"), PathBuf::from("/other")],
    );

    assert!(is_overlap_error(
        &settings.validate_torrent_folder(&paths().torrent_dir())
    ));
}

#[test]
fn a_parent_of_a_library_folder_is_rejected() {
    let settings = with_folders(
        Some(PathBuf::from("/media")),
        vec![PathBuf::from("/media/anime")],
    );

    assert!(is_overlap_error(
        &settings.validate_torrent_folder(&paths().torrent_dir())
    ));
}

#[test]
fn a_child_of_a_library_folder_is_rejected() {
    let settings = with_folders(
        Some(PathBuf::from("/media/anime/season 1")),
        vec![PathBuf::from("/media/anime")],
    );

    assert!(is_overlap_error(
        &settings.validate_torrent_folder(&paths().torrent_dir())
    ));
}

#[test]
fn an_unrelated_folder_is_accepted() {
    let settings = with_folders(
        Some(PathBuf::from("/cache/torrents")),
        vec![
            PathBuf::from("/media/anime"),
            PathBuf::from("/media/movies"),
        ],
    );

    assert!(
        settings
            .validate_torrent_folder(&paths().torrent_dir())
            .is_ok(),
        "a sibling of the library folders is fine"
    );
}

#[test]
fn the_default_folder_is_checked_against_library_folders_too() {
    let paths = paths();
    let settings = with_folders(None, vec![paths.torrent_dir()]);

    assert!(is_overlap_error(
        &settings.validate_torrent_folder(&paths.torrent_dir())
    ));
}

#[test]
fn the_torrent_tab_round_trips_through_rows() {
    let settings = with_folders(
        Some(PathBuf::from("/cache/streams")),
        vec![PathBuf::from("/media/anime")],
    );
    let settings = Settings {
        torrent: TorrentSettings {
            disable_upload: true,
            ..settings.torrent
        },
        ..settings
    };

    let rows = settings.to_rows().unwrap();
    assert!(
        rows.iter().any(|(tab, _)| tab == "torrent"),
        "the torrent tab gets its own row"
    );

    let (loaded, errors) = Settings::from_rows(rows);
    assert!(errors.is_empty(), "{errors:?}");
    assert_eq!(loaded, settings);
}
