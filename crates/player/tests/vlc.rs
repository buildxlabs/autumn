//! Manual VLC check. Run with:
//! `cargo test -p player --test vlc -- --ignored --nocapture`

use std::{error::Error, path::Path, process::Command as StdCommand, time::Duration};

use player::{LaunchOptions, PlayerEvent, launch_vlc};
use tokio::time::timeout;

fn write_fixture(path: &Path) -> Result<(), Box<dyn Error>> {
    let sample_rate = 8_000_u32;
    let data_len = sample_rate * 2 * 2;
    let len = usize::try_from(data_len)?;
    let mut bytes = Vec::with_capacity(44 + len);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_len).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16_u32.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&1_u16.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * 2).to_le_bytes());
    bytes.extend_from_slice(&2_u16.to_le_bytes());
    bytes.extend_from_slice(&16_u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_len.to_le_bytes());
    bytes.resize(44 + len, 0);
    std::fs::write(path, bytes)?;
    Ok(())
}

#[tokio::test]
#[ignore = "needs VLC installed"]
async fn vlc_reports_media_ready_seek_and_eof() -> Result<(), Box<dyn Error>> {
    if !StdCommand::new("vlc")
        .arg("--version")
        .output()?
        .status
        .success()
    {
        eprintln!("VLC is not installed; nothing to check");
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let media = dir.path().join("fixture.wav");
    write_fixture(&media)?;
    let mut session = launch_vlc(
        "vlc",
        &media.to_string_lossy(),
        &LaunchOptions::new(dir.path()),
    )
    .await?;

    let ready = timeout(Duration::from_secs(15), async {
        loop {
            match session.next_event().await {
                Some(PlayerEvent::MediaReady { .. }) => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await?;
    if !ready {
        return Err(std::io::Error::other("VLC never reported MediaReady").into());
    }

    session.seek(0.25).await?;
    let seek_seen = timeout(Duration::from_secs(5), async {
        loop {
            match session.next_event().await {
                Some(PlayerEvent::Position { seconds }) if seconds >= 0.25 => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await?;
    if !seek_seen {
        return Err(std::io::Error::other("VLC did not report the seek").into());
    }

    let eof = timeout(Duration::from_secs(20), async {
        loop {
            match session.next_event().await {
                Some(PlayerEvent::Eof) => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await?;
    if !eof {
        return Err(std::io::Error::other("VLC never reached EOF").into());
    }

    session.quit().await.ok();
    Ok(())
}
