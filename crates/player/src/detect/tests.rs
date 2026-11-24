//! Layout tests with fake roots, plus a manual check of the real machine.

use std::path::Path;

use super::*;
use crate::Error;

fn name(binary: &str, platform: Platform) -> String {
    if platform == Platform::Windows {
        format!("{binary}.exe")
    } else {
        binary.to_owned()
    }
}

fn place(root: &Path, relative: &str) {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, b"fake player").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
}

fn kinds(players: &[InstalledPlayer]) -> Vec<PlayerKind> {
    players.iter().map(|player| player.kind).collect()
}

#[tokio::test]
async fn macos_layout_finds_each_player_and_prefers_iina() {
    let dir = tempfile::tempdir().unwrap();
    let env = SearchEnv::fake(Platform::Macos, dir.path());
    place(dir.path(), &format!("bin/{}", name("mpv", env.platform)));
    place(dir.path(), "Applications/IINA.app/Contents/MacOS/iina-cli");
    place(dir.path(), "Applications/VLC.app/Contents/MacOS/VLC");

    let players = detect(&env).await;

    assert_eq!(
        kinds(&players),
        vec![PlayerKind::Iina, PlayerKind::Mpv, PlayerKind::Vlc]
    );
    assert_eq!(
        preferred(&players, Platform::Macos).map(|player| player.kind),
        Some(PlayerKind::Iina)
    );
}

#[tokio::test]
async fn linux_layout_uses_flatpak_for_mpv_and_prefers_it() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = SearchEnv::fake(Platform::Linux, dir.path());
    place(
        dir.path(),
        &format!("usr/bin/{}", name("vlc", env.platform)),
    );
    env.flatpak_apps
        .push(("io.mpv.Mpv".to_owned(), Some("0.40.0".to_owned())));

    let players = detect(&env).await;

    assert_eq!(kinds(&players), vec![PlayerKind::Mpv, PlayerKind::Vlc]);
    assert_eq!(
        players.first().map(|player| &player.command),
        Some(&LaunchCommand::Flatpak("io.mpv.Mpv".to_owned()))
    );
    assert_eq!(
        players.first().and_then(|player| player.version.as_deref()),
        Some("0.40.0")
    );
    assert_eq!(
        preferred(&players, Platform::Linux).map(|player| player.kind),
        Some(PlayerKind::Mpv)
    );
}

#[tokio::test]
async fn windows_layout_finds_each_player_and_prefers_mpv() {
    let dir = tempfile::tempdir().unwrap();
    let env = SearchEnv::fake(Platform::Windows, dir.path());
    place(dir.path(), "WinGet/Links/mpv.exe");
    place(dir.path(), "Program Files/VideoLAN/VLC/vlc.exe");
    place(dir.path(), "Program Files/MPC-HC/mpc-hc.exe");

    let players = detect(&env).await;

    assert_eq!(
        kinds(&players),
        vec![PlayerKind::Mpv, PlayerKind::MpcHc, PlayerKind::Vlc]
    );
    assert_eq!(
        preferred(&players, Platform::Windows).map(|player| player.kind),
        Some(PlayerKind::Mpv)
    );
}

#[tokio::test]
async fn windows_registry_install_dirs_are_searched() {
    let dir = tempfile::tempdir().unwrap();
    let env = SearchEnv::fake(Platform::Windows, dir.path());
    place(dir.path(), "Registry/VLC/vlc.exe");
    place(dir.path(), "Registry/MPC-HC/mpc-hc.exe");

    let players = detect(&env).await;

    assert_eq!(kinds(&players), vec![PlayerKind::MpcHc, PlayerKind::Vlc]);
}

#[tokio::test]
async fn an_injected_version_reaches_the_result_without_spawning() {
    let dir = tempfile::tempdir().unwrap();
    let mut env = SearchEnv::fake(Platform::Linux, dir.path());
    let mpv = dir.path().join("bin/mpv");
    place(dir.path(), "bin/mpv");
    env.versions.insert(mpv, "mpv 0.40.0".to_owned());

    let players = detect(&env).await;

    assert_eq!(
        players.first().and_then(|player| player.version.as_deref()),
        Some("mpv 0.40.0")
    );
}

#[tokio::test]
async fn a_missing_custom_path_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let env = SearchEnv::fake(Platform::Linux, dir.path());

    let result = validate_custom_path(&env, &dir.path().join("bin/nope")).await;

    assert!(matches!(result, Err(Error::NotFound { .. })));
}

#[tokio::test]
async fn a_non_executable_custom_path_is_a_typed_error() {
    let dir = tempfile::tempdir().unwrap();
    let env = SearchEnv::fake(Platform::Linux, dir.path());
    let path = dir.path().join("bin/mpv");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).unwrap();
    }
    std::fs::write(&path, b"not executable").unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
    }

    let result = validate_custom_path(&env, &path).await;

    assert!(matches!(result, Err(Error::NotExecutable { .. })));
}

#[test]
fn registry_values_point_at_player_directories() {
    assert_eq!(
        registry_root("C:/Program Files/VideoLAN/VLC"),
        Path::new("C:/Program Files/VideoLAN/VLC")
    );
    assert_eq!(
        registry_root("C:/Program Files/MPC-HC/mpc-hc64.exe"),
        Path::new("C:/Program Files/MPC-HC")
    );
    assert_eq!(
        registry_root("D:/Programs/MPC-HC/MPC-HC.EXE"),
        Path::new("D:/Programs/MPC-HC")
    );
}

#[tokio::test]
async fn a_valid_custom_path_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let env = SearchEnv::fake(Platform::Linux, dir.path());
    let path = dir.path().join("bin/mpv");
    place(dir.path(), "bin/mpv");

    let player = validate_custom_path(&env, &path).await.unwrap();

    assert_eq!(player.kind, PlayerKind::Mpv);
    assert_eq!(player.command, LaunchCommand::Executable(path));
}

#[tokio::test]
async fn every_declared_search_root_finds_a_player() {
    for (platform, root) in [
        (Platform::Macos, "opt/homebrew/bin"),
        (Platform::Macos, "usr/local/bin"),
        (Platform::Macos, "opt/local/bin"),
        (Platform::Macos, "nix/var/nix/profiles/default/bin"),
        (Platform::Linux, "usr/local/bin"),
        (Platform::Linux, "snap/bin"),
        (Platform::Linux, "nix/var/nix/profiles/default/bin"),
        (Platform::Windows, "scoop/shims"),
        (Platform::Windows, "chocolatey/bin"),
        (Platform::Windows, "WinGet/Links"),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let env = SearchEnv::fake(platform, dir.path());
        let binary = format!("{root}/{}", name("mpv", platform));
        place(dir.path(), &binary);

        let players = detect(&env).await;

        assert_eq!(
            players.first().map(|player| &player.command),
            Some(&LaunchCommand::Executable(dir.path().join(&binary))),
            "root {root} on {platform:?}"
        );
    }
}

#[tokio::test]
#[ignore = "logs the real players installed on this machine"]
async fn list_real_installed_players() {
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .finish();
    let _ = tracing::subscriber::set_global_default(subscriber);
    let env = SearchEnv::system().await;
    let players = detect(&env).await;
    for player in players {
        tracing::info!(
            "{}: {} ({})",
            player.kind.label(),
            player.command.display(),
            player.version.as_deref().unwrap_or("unknown version")
        );
    }
}
