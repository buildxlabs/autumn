//! Player detection on macOS, Linux and Windows.
//!
//! Search roots and the environment are injected through [`SearchEnv`], so
//! the same layout tests run on every OS. `SearchEnv::system` uses the real
//! machine; tests build a fake layout in a temp dir.

use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    time::Duration,
};

use tokio::process::Command;

use crate::Error;

const VERSION_TIMEOUT: Duration = Duration::from_secs(2);

/// The operating system whose layout to search. Injected for tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Platform {
    /// macOS app bundles, Homebrew, `MacPorts` and Nix profiles.
    Macos,
    /// PATH, system bin dirs, Flatpak and Snap.
    Linux,
    /// Program Files, `WinGet`, Scoop, Chocolatey and the registry.
    Windows,
}

impl Platform {
    /// The platform this binary is running on.
    #[must_use]
    pub fn host() -> Self {
        #[cfg(target_os = "macos")]
        return Self::Macos;
        #[cfg(target_os = "linux")]
        return Self::Linux;
        #[cfg(target_os = "windows")]
        return Self::Windows;
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
        return Self::Linux;
    }

    /// The player kinds to prefer, best first.
    #[must_use]
    pub fn preference(self) -> &'static [PlayerKind] {
        match self {
            Self::Macos => &[PlayerKind::Iina, PlayerKind::Mpv, PlayerKind::Vlc],
            Self::Linux => &[PlayerKind::Mpv, PlayerKind::Vlc],
            Self::Windows => &[PlayerKind::Mpv, PlayerKind::MpcHc, PlayerKind::Vlc],
        }
    }
}

/// The player families Autumn can drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum PlayerKind {
    /// mpv, over JSON IPC.
    Mpv,
    /// IINA on macOS, over its embedded mpv's JSON IPC.
    Iina,
    /// VLC, over its HTTP interface.
    Vlc,
    /// MPC-HC on Windows, over its web interface.
    MpcHc,
}

impl PlayerKind {
    /// The name shown in settings.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Mpv => "mpv",
            Self::Iina => "IINA",
            Self::Vlc => "VLC",
            Self::MpcHc => "MPC-HC",
        }
    }

    fn supported(self, platform: Platform) -> bool {
        match self {
            Self::Iina => platform == Platform::Macos,
            Self::MpcHc => platform == Platform::Windows,
            Self::Mpv | Self::Vlc => true,
        }
    }

    fn bin_names(self, platform: Platform) -> &'static [&'static str] {
        match (self, platform) {
            (Self::Mpv, Platform::Windows) => &["mpv.exe", "mpv.com"],
            (Self::Iina, Platform::Windows) => &["iina-cli.exe"],
            (Self::Vlc, Platform::Windows) => &["vlc.exe"],
            (Self::MpcHc, Platform::Windows) => &["mpc-hc.exe", "mpc-hc64.exe"],
            (Self::Mpv, _) => &["mpv"],
            (Self::Iina, _) => &["iina-cli", "iina"],
            (Self::Vlc, _) => &["vlc"],
            (Self::MpcHc, _) => &["mpc-hc", "mpc-hc64"],
        }
    }

    fn flatpak_id(self) -> Option<&'static str> {
        match self {
            Self::Mpv => Some("io.mpv.Mpv"),
            Self::Vlc => Some("org.videolan.VLC"),
            Self::Iina | Self::MpcHc => None,
        }
    }
}

/// How to start a detected player.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LaunchCommand {
    /// Start this executable directly.
    Executable(PathBuf),
    /// Start a Flatpak app with `flatpak run <app id>`.
    Flatpak(String),
}

impl LaunchCommand {
    /// A display form for settings and logs.
    #[must_use]
    pub fn display(&self) -> String {
        match self {
            Self::Executable(path) => path.to_string_lossy().into_owned(),
            Self::Flatpak(id) => format!("flatpak run {id}"),
        }
    }
}

/// A player found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledPlayer {
    /// Which player this is.
    pub kind: PlayerKind,
    /// The command that starts it.
    pub command: LaunchCommand,
    /// The first line of `--version`, when it could be read.
    pub version: Option<String>,
}

/// The environment detection reads. Every field is injected for tests.
#[derive(Debug, Clone)]
pub struct SearchEnv {
    /// The platform whose layout to search.
    pub platform: Platform,
    /// `PATH` entries to search for bare binary names.
    pub path: Vec<PathBuf>,
    /// Extra binary directories (Homebrew, `MacPorts`, Nix, `WinGet`, Scoop, ...).
    pub roots: Vec<PathBuf>,
    /// Explicit app-bundle or install locations.
    pub bundles: Vec<(PlayerKind, PathBuf)>,
    /// Flatpak app ids and versions reported by `flatpak info`.
    pub flatpak_apps: Vec<(String, Option<String>)>,
    /// Install directories read from the Windows registry.
    pub registry_roots: Vec<PathBuf>,
    /// Version output by path, so tests never spawn fake executables.
    pub versions: HashMap<PathBuf, String>,
    /// Whether to probe real executables with `--version`.
    pub probe_versions: bool,
}

/// Binary roots and app bundles for a real machine.
fn system_layout(
    platform: Platform,
    home: Option<&Path>,
) -> (Vec<PathBuf>, Vec<(PlayerKind, PathBuf)>) {
    let root = if cfg!(windows) {
        std::env::var_os("SystemDrive").map_or_else(|| PathBuf::from("C:\\"), PathBuf::from)
    } else {
        PathBuf::from("/")
    };

    let mut roots = Vec::new();
    let mut bundles = Vec::new();
    match platform {
        Platform::Macos => {
            for bin in ["opt/homebrew/bin", "usr/local/bin", "opt/local/bin"] {
                roots.push(root.join(bin));
            }
            if let Some(home) = home {
                roots.push(home.join(".nix-profile/bin"));
                bundles.push((
                    PlayerKind::Iina,
                    home.join("Applications/IINA.app/Contents/MacOS/iina-cli"),
                ));
                bundles.push((
                    PlayerKind::Mpv,
                    home.join("Applications/mpv.app/Contents/MacOS/mpv"),
                ));
                bundles.push((
                    PlayerKind::Vlc,
                    home.join("Applications/VLC.app/Contents/MacOS/VLC"),
                ));
            }
            roots.push(root.join("run/current-system/sw/bin"));
            roots.push(root.join("nix/var/nix/profiles/default/bin"));
            for (kind, rel) in [
                (
                    PlayerKind::Iina,
                    "Applications/IINA.app/Contents/MacOS/iina-cli",
                ),
                (PlayerKind::Mpv, "Applications/mpv.app/Contents/MacOS/mpv"),
                (PlayerKind::Vlc, "Applications/VLC.app/Contents/MacOS/VLC"),
            ] {
                bundles.push((kind, root.join(rel)));
            }
        }
        Platform::Linux => {
            for bin in [
                "usr/bin",
                "usr/local/bin",
                "bin",
                "snap/bin",
                "run/current-system/sw/bin",
            ] {
                roots.push(root.join(bin));
            }
            if let Some(home) = home {
                roots.push(home.join(".nix-profile/bin"));
                roots.push(home.join(".local/bin"));
            }
            roots.push(root.join("nix/var/nix/profiles/default/bin"));
        }
        Platform::Windows => {
            let program_files = std::env::var_os("ProgramFiles").map(PathBuf::from);
            let program_files_x86 = std::env::var_os("ProgramFiles(x86)").map(PathBuf::from);
            let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
            for base in [program_files, program_files_x86].into_iter().flatten() {
                roots.push(base.join("mpv"));
                roots.push(base.join("VideoLAN\\VLC"));
                roots.push(base.join("MPC-HC"));
            }
            if let Some(local) = local {
                roots.push(local.join("Microsoft\\WinGet\\Links"));
                roots.push(local.join("Programs\\mpv"));
                roots.push(local.join("Programs\\VideoLAN\\VLC"));
            }
            if let Some(home) = home {
                roots.push(home.join("scoop\\shims"));
                roots.push(home.join("scoop\\apps\\mpv\\current"));
                roots.push(home.join("scoop\\apps\\vlc\\current"));
            }
            let chocolatey = std::env::var_os("ProgramData")
                .map_or_else(|| root.join("ProgramData"), PathBuf::from);
            roots.push(chocolatey.join("chocolatey\\bin"));
        }
    }
    (roots, bundles)
}

impl SearchEnv {
    /// Reads the real machine: `PATH`, home and each platform's install roots.
    #[must_use]
    pub async fn system() -> Self {
        let platform = Platform::host();
        let path =
            std::env::var_os("PATH").map_or_default(|raw| std::env::split_paths(&raw).collect());
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from);
        let (roots, bundles) = system_layout(platform, home.as_deref());

        let flatpak_apps = if platform == Platform::Linux {
            probe_flatpak().await
        } else {
            Vec::new()
        };

        Self {
            platform,
            path,
            roots,
            bundles,
            flatpak_apps,
            registry_roots: registry_install_dirs(),
            versions: HashMap::new(),
            probe_versions: true,
        }
    }

    /// A layout rooted at `root`, with no real process probing.
    #[must_use]
    pub fn fake(platform: Platform, root: &Path) -> Self {
        let bin = |name: &str| root.join(name);
        let bundle = |kind: PlayerKind, name: &str| (kind, root.join(name));
        let mut roots = vec![
            bin("bin"),
            bin("usr/bin"),
            bin("usr/local/bin"),
            bin("opt/homebrew/bin"),
            bin("opt/local/bin"),
            bin("snap/bin"),
            bin(".nix-profile/bin"),
            bin(".local/bin"),
            bin("nix/var/nix/profiles/default/bin"),
        ];
        if platform == Platform::Windows {
            roots.push(bin("Program Files/mpv"));
            roots.push(bin("Program Files/VideoLAN/VLC"));
            roots.push(bin("Program Files/MPC-HC"));
            roots.push(bin("WinGet/Links"));
            roots.push(bin("scoop/shims"));
            roots.push(bin("scoop/apps/mpv/current"));
            roots.push(bin("scoop/apps/vlc/current"));
            roots.push(bin("chocolatey/bin"));
        }
        Self {
            platform,
            path: vec![bin("bin")],
            roots,
            bundles: vec![
                bundle(
                    PlayerKind::Iina,
                    "Applications/IINA.app/Contents/MacOS/iina-cli",
                ),
                bundle(PlayerKind::Mpv, "Applications/mpv.app/Contents/MacOS/mpv"),
                bundle(PlayerKind::Vlc, "Applications/VLC.app/Contents/MacOS/VLC"),
            ],
            flatpak_apps: Vec::new(),
            registry_roots: if platform == Platform::Windows {
                vec![bin("Registry/VLC"), bin("Registry/MPC-HC")]
            } else {
                Vec::new()
            },
            versions: HashMap::new(),
            probe_versions: false,
        }
    }
}

async fn probe_flatpak() -> Vec<(String, Option<String>)> {
    let mut found = Vec::new();
    for id in ["io.mpv.Mpv", "org.videolan.VLC"] {
        let mut probe = Command::new("flatpak");
        probe
            .kill_on_drop(true)
            .args(["info", id])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null());
        let installed = tokio::time::timeout(VERSION_TIMEOUT, probe.status()).await;
        if !matches!(installed, Ok(Ok(status)) if status.success()) {
            continue;
        }

        let mut version_probe = Command::new("flatpak");
        version_probe
            .kill_on_drop(true)
            .args(["run", id, "--version"])
            .stderr(std::process::Stdio::null());
        let version = tokio::time::timeout(VERSION_TIMEOUT, version_probe.output())
            .await
            .ok()
            .and_then(Result::ok)
            .filter(|output| output.status.success())
            .and_then(|output| {
                String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .next()
                    .map(str::trim)
                    .filter(|line| !line.is_empty())
                    .map(str::to_owned)
            });
        found.push((id.to_owned(), version));
    }
    found
}

/// Turns a registry install value into a directory to search.
///
/// MPC-HC's `ExePath` names the executable itself, while VLC's `InstallDir`
/// and the uninstall key's `InstallLocation` are already directories.
#[cfg(any(windows, test))]
fn registry_root(value: &str) -> PathBuf {
    let path = PathBuf::from(value);
    let is_executable = path
        .extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("com"));
    if is_executable {
        match path.parent() {
            Some(parent) => parent.to_path_buf(),
            None => path,
        }
    } else {
        path
    }
}

#[cfg(windows)]
fn registry_install_dirs() -> Vec<PathBuf> {
    use winreg::{
        RegKey,
        enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE},
    };

    let mut roots = Vec::new();
    for hive in [HKEY_LOCAL_MACHINE, HKEY_CURRENT_USER] {
        for (key, value) in [
            (r"SOFTWARE\VideoLAN\VLC", "InstallDir"),
            (r"SOFTWARE\WOW6432Node\VideoLAN\VLC", "InstallDir"),
            (r"SOFTWARE\MPC-HC\MPC-HC", "ExePath"),
            (r"SOFTWARE\WOW6432Node\MPC-HC\MPC-HC", "ExePath"),
            (
                r"SOFTWARE\Microsoft\Windows\CurrentVersion\Uninstall\MPC-HC",
                "InstallLocation",
            ),
            (
                r"SOFTWARE\WOW6432Node\Microsoft\Windows\CurrentVersion\Uninstall\MPC-HC",
                "InstallLocation",
            ),
        ] {
            if let Ok(subkey) = RegKey::predef(hive).open_subkey(key)
                && let Ok(value) = subkey.get_value::<String, _>(value)
            {
                roots.push(registry_root(&value));
            }
        }
    }
    roots
}

#[cfg(not(windows))]
fn registry_install_dirs() -> Vec<PathBuf> {
    Vec::new()
}

/// Finds installed players and returns them in the platform's default order.
#[must_use]
pub async fn detect(env: &SearchEnv) -> Vec<InstalledPlayer> {
    let mut found = Vec::new();
    for kind in [
        PlayerKind::Mpv,
        PlayerKind::Iina,
        PlayerKind::Vlc,
        PlayerKind::MpcHc,
    ] {
        if !kind.supported(env.platform) {
            continue;
        }
        if let Some(player) = find_kind(kind, env).await {
            found.push(player);
        }
    }
    found.sort_by_key(|player| {
        env.platform
            .preference()
            .iter()
            .position(|kind| *kind == player.kind)
            .unwrap_or(usize::MAX)
    });
    found
}

async fn find_kind(kind: PlayerKind, env: &SearchEnv) -> Option<InstalledPlayer> {
    for dir in env
        .path
        .iter()
        .chain(env.roots.iter())
        .chain(env.registry_roots.iter())
    {
        for name in kind.bin_names(env.platform) {
            let path = dir.join(name);
            if is_executable(env, &path) {
                return Some(InstalledPlayer {
                    kind,
                    command: LaunchCommand::Executable(path.clone()),
                    version: version_of(env, &path).await,
                });
            }
        }
    }
    for (bundled, path) in &env.bundles {
        if *bundled == kind && is_executable(env, path) {
            return Some(InstalledPlayer {
                kind,
                command: LaunchCommand::Executable(path.clone()),
                version: version_of(env, path).await,
            });
        }
    }
    if env.platform == Platform::Linux
        && let Some(id) = kind.flatpak_id()
        && let Some((installed, version)) = env
            .flatpak_apps
            .iter()
            .find(|(installed, _)| installed == id)
    {
        return Some(InstalledPlayer {
            kind,
            command: LaunchCommand::Flatpak(installed.clone()),
            version: version.clone(),
        });
    }
    None
}

async fn version_of(env: &SearchEnv, path: &Path) -> Option<String> {
    if let Some(version) = env.versions.get(path) {
        return Some(version.clone());
    }
    if !env.probe_versions {
        return None;
    }
    let mut probe = Command::new(path);
    probe.kill_on_drop(true).arg("--version");
    let output = tokio::time::timeout(VERSION_TIMEOUT, probe.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .next()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
}

fn is_executable(env: &SearchEnv, path: &Path) -> bool {
    if !path.is_file() {
        return false;
    }
    if env.platform == Platform::Windows {
        #[cfg(windows)]
        {
            use std::io::Read;

            let Some(ext) = path.extension().and_then(|ext| ext.to_str()) else {
                return false;
            };
            let batch = ext.eq_ignore_ascii_case("bat") || ext.eq_ignore_ascii_case("cmd");
            if !(batch || ext.eq_ignore_ascii_case("exe") || ext.eq_ignore_ascii_case("com")) {
                return false;
            }
            if batch {
                return true;
            }
            // Windows paths ignore case and a file named `mpv.exe` may still
            // be an arbitrary text file; require the PE signature.
            let Ok(mut file) = std::fs::File::open(path) else {
                return false;
            };
            let mut magic = [0_u8; 2];
            return file.read_exact(&mut magic).is_ok() && &magic == b"MZ";
        }
        #[cfg(not(windows))]
        {
            return path
                .extension()
                .and_then(|ext| ext.to_str())
                .is_some_and(|ext| {
                    ["exe", "com", "bat", "cmd"]
                        .iter()
                        .any(|kind| ext.eq_ignore_ascii_case(kind))
                });
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.metadata()
            .is_ok_and(|meta| meta.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        true
    }
}

/// Suggests the best installed player for the platform, if any.
#[must_use]
pub fn preferred(players: &[InstalledPlayer], platform: Platform) -> Option<&InstalledPlayer> {
    platform
        .preference()
        .iter()
        .find_map(|kind| players.iter().find(|player| player.kind == *kind))
}

/// Validates a user-entered player path with the same checks as detection.
///
/// # Errors
///
/// Returns [`Error::NotFound`] if the path does not exist,
/// [`Error::NotExecutable`] if it cannot be run, or [`Error::Unrecognized`]
/// if its name does not match a supported player.
pub async fn validate_custom_path(env: &SearchEnv, path: &Path) -> Result<InstalledPlayer, Error> {
    if !path.is_file() {
        return Err(Error::NotFound {
            path: path.to_path_buf(),
        });
    }
    if !is_executable(env, path) {
        return Err(Error::NotExecutable {
            path: path.to_path_buf(),
        });
    }
    let file = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let kind = if file.contains("mpv") {
        PlayerKind::Mpv
    } else if file.contains("iina") {
        PlayerKind::Iina
    } else if file.contains("vlc") {
        PlayerKind::Vlc
    } else if file.contains("mpc-hc") || file.contains("mpchc") {
        PlayerKind::MpcHc
    } else {
        return Err(Error::Unrecognized {
            path: path.to_path_buf(),
        });
    };
    if !kind.supported(env.platform) {
        return Err(Error::Unrecognized {
            path: path.to_path_buf(),
        });
    }
    Ok(InstalledPlayer {
        kind,
        command: LaunchCommand::Executable(path.to_path_buf()),
        version: version_of(env, path).await,
    })
}

#[cfg(test)]
mod tests;
