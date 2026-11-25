//! The composition-root side of Settings → Playback.
//!
//! Detection, custom-path validation and the Test button all need the Tokio
//! runtime and the `player` crate, so they live in `autumn` and reach the
//! screen as plain callbacks. The screen never names `player` or the engine.

use std::{cell::RefCell, path::Path, path::PathBuf};

use db::Db;
use gpui_kit::App;
use player::{
    Error as PlayerError, InstalledPlayer, LaunchCommand, LaunchOptions, PlayerKind, SearchEnv,
    Session, detect, launch_iina, launch_mpchc, launch_mpv, launch_vlc, preferred,
    validate_custom_path,
};
use settings::{PlaybackKind, PlaybackSettings, PlayerChoice, Settings};
use ui_screens::{PlaybackServices, PlaybackTestError, PlayerOption};

/// Only the Playback row is written; other tabs are saved by their own screens.
const PLAYBACK_TAB: &str = "playback";
/// Length of the silent clip the Test button opens, in seconds.
const CLIP_SECONDS: u32 = 2;
/// Block align of a 16-bit mono WAV.
const BLOCK_ALIGN: u16 = 2;

fn to_setting_kind(kind: PlayerKind) -> Option<PlaybackKind> {
    match kind {
        PlayerKind::Mpv => Some(PlaybackKind::Mpv),
        PlayerKind::Iina => Some(PlaybackKind::Iina),
        PlayerKind::Vlc => Some(PlaybackKind::Vlc),
        PlayerKind::MpcHc => Some(PlaybackKind::MpcHc),
        // A player kind settings can't name yet is not offered for selection.
        _ => None,
    }
}

fn to_player_kind(kind: PlaybackKind) -> PlayerKind {
    match kind {
        PlaybackKind::Mpv => PlayerKind::Mpv,
        PlaybackKind::Iina => PlayerKind::Iina,
        PlaybackKind::Vlc => PlayerKind::Vlc,
        PlaybackKind::MpcHc => PlayerKind::MpcHc,
    }
}

fn to_option(player: InstalledPlayer) -> Option<PlayerOption> {
    let kind = to_setting_kind(player.kind)?;
    let label = match player.version {
        Some(version) => format!("{} — {version}", player.kind.label()),
        None => player.kind.label().to_owned(),
    };
    let path = match player.command {
        LaunchCommand::Executable(path) => path,
        LaunchCommand::Flatpak(id) => PathBuf::from(id),
        _ => return None,
    };
    Some(PlayerOption {
        kind,
        label: label.into(),
        path,
    })
}

fn map_error(error: PlayerError) -> PlaybackTestError {
    match error {
        PlayerError::WebInterfaceDisabled => PlaybackTestError::WebInterfaceDisabled,
        other => PlaybackTestError::Failed(other.to_string()),
    }
}

/// Builds the callbacks the Playback tab calls through the Tokio bridge.
pub(crate) fn services(db: Db, settings: Settings, cache_dir: &Path) -> PlaybackServices {
    let current = RefCell::new(settings);
    let save = move |playback: &PlaybackSettings, cx: &mut App| {
        let rows = {
            let mut settings = current.borrow_mut();
            settings.playback = playback.clone();
            match settings.to_rows() {
                Ok(rows) => rows
                    .into_iter()
                    .filter(|(tab, _)| tab == PLAYBACK_TAB)
                    .collect::<Vec<_>>(),
                Err(error) => {
                    tracing::warn!(%error, "could not encode the playback settings");
                    Vec::new()
                }
            }
        };
        let db = db.clone();
        let write = ui_state::bridge::run(cx, async move {
            if rows.is_empty() {
                return Ok(());
            }
            db.settings().save(&rows).await
        });
        cx.background_executor().spawn(async move {
            if let Err(error) = write.await {
                tracing::warn!(%error, "playback settings were not saved");
            }
        })
    };

    let detect = |cx: &mut App| {
        let scan = ui_state::bridge::run(cx, async {
            let env = SearchEnv::system().await;
            detect(&env)
                .await
                .into_iter()
                .filter_map(to_option)
                .collect()
        });
        cx.background_executor().spawn(async move {
            match scan.await {
                Ok(players) => players,
                Err(error) => {
                    tracing::warn!(%error, "player detection failed");
                    Vec::new()
                }
            }
        })
    };

    let validate = |path: PathBuf, cx: &mut App| {
        let checked = ui_state::bridge::run(cx, async move {
            let env = SearchEnv::system().await;
            validate_custom_path(&env, &path)
                .await
                .map_err(|error| error.to_string())
                .and_then(|player| to_option(player).ok_or_else(|| "unsupported player".to_owned()))
        });
        cx.background_executor().spawn(async move {
            match checked.await {
                Ok(result) => result,
                Err(error) => Err(error.to_string()),
            }
        })
    };

    let cache_dir = cache_dir.to_path_buf();
    let test = move |settings: PlaybackSettings, cx: &mut App| {
        let cache_dir = cache_dir.clone();
        let launch =
            ui_state::bridge::run(cx, async move { test_launch(&settings, &cache_dir).await });
        cx.background_executor().spawn(async move {
            match launch.await {
                Ok(result) => result,
                Err(error) => Err(PlaybackTestError::Failed(error.to_string())),
            }
        })
    };

    PlaybackServices::new(save, detect, validate, test)
}

/// Launches the selected player on a short silent clip and closes it again.
/// Reaching a connected session means the player and its control interface
/// work; MPC-HC's refused web interface is reported with its own message.
async fn test_launch(
    settings: &PlaybackSettings,
    cache_dir: &Path,
) -> Result<(), PlaybackTestError> {
    let env = SearchEnv::system().await;
    let players = detect(&env).await;
    let selected = match &settings.player {
        PlayerChoice::Auto => preferred(&players, env.platform).cloned(),
        PlayerChoice::Installed { kind } => players
            .iter()
            .find(|player| to_setting_kind(player.kind) == Some(*kind))
            .cloned(),
        PlayerChoice::Custom { kind, path } => Some(InstalledPlayer {
            kind: to_player_kind(*kind),
            command: LaunchCommand::Executable(path.clone()),
            version: None,
        }),
    };
    let player = selected.ok_or_else(|| {
        PlaybackTestError::Invalid("the selected player is not installed".to_owned())
    })?;
    let executable = match &player.command {
        LaunchCommand::Executable(path) => path.clone(),
        LaunchCommand::Flatpak(_) => {
            return Err(PlaybackTestError::Failed(
                "Flatpak players cannot be tested from Settings yet".to_owned(),
            ));
        }
        _ => {
            return Err(PlaybackTestError::Failed(
                "unsupported launch command".to_owned(),
            ));
        }
    };

    let clip = test_clip(cache_dir)?;
    let file = clip.to_string_lossy().into_owned();
    let options = if player.kind == PlayerKind::Iina {
        LaunchOptions::iina(cache_dir)
    } else {
        LaunchOptions::new(cache_dir)
    };

    let session: Session = match player.kind {
        PlayerKind::Mpv => launch_mpv(&executable, &file, &options).map_err(map_error)?,
        PlayerKind::Iina => launch_iina(&executable, &file, &options).map_err(map_error)?,
        PlayerKind::Vlc => launch_vlc(&executable, &file, &options)
            .await
            .map_err(map_error)?,
        PlayerKind::MpcHc => launch_mpchc(
            &executable,
            &settings.mpchc_host,
            settings.mpchc_port,
            &file,
            &options,
        )
        .await
        .map_err(map_error)?,
        _ => {
            return Err(PlaybackTestError::Failed("unsupported player".to_owned()));
        }
    };
    // mpv can close its socket before replying, and `quit` treats that as
    // success. The supervisor reaps the detached process after this returns.
    let _ = session.quit().await;
    Ok(())
}

/// Writes a small silent WAV so Test needs no bundled binary asset. A WAV is
/// played by all four players without codec setup.
fn test_clip(cache_dir: &Path) -> Result<PathBuf, PlaybackTestError> {
    const SAMPLE_RATE: u32 = 8_000;
    const SAMPLE_WIDTH: u32 = 2;
    let data_len = SAMPLE_RATE * CLIP_SECONDS * SAMPLE_WIDTH;
    let len = usize::try_from(data_len)
        .map_err(|_| PlaybackTestError::Failed("the test clip is too large".to_owned()))?;
    let mut bytes = Vec::with_capacity(44 + len);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
    bytes.extend_from_slice(&(SAMPLE_RATE * SAMPLE_WIDTH).to_le_bytes());
    bytes.extend_from_slice(&BLOCK_ALIGN.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(44 + len, 0);
    let path = cache_dir.join("autumn-test-clip.wav");
    std::fs::write(&path, bytes).map_err(|error| {
        PlaybackTestError::Failed(format!("could not write the test clip: {error}"))
    })?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_test_clip_is_a_two_second_silent_wav() {
        let dir = tempfile::tempdir().unwrap();
        let path = test_clip(dir.path()).unwrap();
        let bytes = std::fs::read(path).unwrap();

        assert_eq!(bytes.get(..4), Some(b"RIFF".as_slice()));
        assert_eq!(bytes.get(8..12), Some(b"WAVE".as_slice()));
        let seconds = u32::from_le_bytes(bytes.get(24..28).unwrap().try_into().unwrap());
        assert_eq!(seconds, 8_000, "sample rate");
        let data_len = u32::from_le_bytes(bytes.get(40..44).unwrap().try_into().unwrap());
        assert_eq!(data_len, 8_000 * CLIP_SECONDS * 2);
        let expected_len = 44 + usize::try_from(data_len).unwrap();
        assert_eq!(bytes.len(), expected_len);
        assert!(
            bytes
                .get(44..)
                .is_some_and(|data| data.iter().all(|byte| *byte == 0)),
            "the clip is silent"
        );
    }
}
