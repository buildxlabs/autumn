//! Manual integration check against a real mpv. Run with:
//! `cargo test -p player --test mpv -- --ignored --nocapture`

#![cfg(unix)]

use std::{error::Error, path::Path, process::Command as StdCommand, time::Duration};

use player::{LaunchOptions, PlayerEvent, Session, launch_mpv};
use tokio::time::{error::Elapsed, timeout};

/// A two-second silent PCM WAV, enough to observe positions and EOF.
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

async fn wait_for(
    session: &mut Session,
    mut matches: impl FnMut(&PlayerEvent) -> bool,
    limit: Duration,
) -> Result<bool, Elapsed> {
    timeout(limit, async {
        loop {
            match session.next_event().await {
                Some(event) if matches(&event) => return true,
                Some(_) => {}
                None => return false,
            }
        }
    })
    .await
}

#[tokio::test]
#[ignore = "needs mpv on PATH and a graphical session"]
async fn a_real_mpv_session_reports_events_and_removes_its_socket() -> Result<(), Box<dyn Error>> {
    if !StdCommand::new("mpv")
        .arg("--version")
        .output()?
        .status
        .success()
    {
        eprintln!("mpv is not installed; nothing to check");
        return Ok(());
    }

    let dir = tempfile::tempdir()?;
    let media = dir.path().join("fixture.wav");
    write_fixture(&media)?;
    let cache = dir.path().join("cache");

    let mut session = launch_mpv("mpv", &media.to_string_lossy(), &LaunchOptions::new(&cache))?;
    let socket = session.endpoint().as_str().to_owned();
    let socket_open = timeout(Duration::from_secs(5), async {
        loop {
            if Path::new(&socket).exists() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await?;
    if !socket_open {
        return Err(std::io::Error::other("mpv never opened the socket").into());
    }

    if !wait_for(
        &mut session,
        |event| matches!(event, PlayerEvent::MediaReady { .. }),
        Duration::from_secs(10),
    )
    .await?
    {
        return Err(std::io::Error::other("MediaReady did not arrive within ten seconds").into());
    }

    session.seek(0.25).await?;
    if !wait_for(
        &mut session,
        |event| matches!(event, PlayerEvent::Position { seconds } if *seconds >= 0.25),
        Duration::from_secs(5),
    )
    .await?
    {
        return Err(std::io::Error::other("the seek was not reported").into());
    }

    session.pause().await?;
    if !wait_for(
        &mut session,
        |event| matches!(event, PlayerEvent::Paused(true)),
        Duration::from_secs(5),
    )
    .await?
    {
        return Err(std::io::Error::other("the pause was not reported").into());
    }
    session.resume().await?;

    if !wait_for(
        &mut session,
        |event| matches!(event, PlayerEvent::Eof),
        Duration::from_secs(15),
    )
    .await?
    {
        return Err(std::io::Error::other("EOF did not arrive within fifteen seconds").into());
    }

    session.quit().await?;
    if !wait_for(
        &mut session,
        |event| matches!(event, PlayerEvent::Exited),
        Duration::from_secs(5),
    )
    .await?
    {
        return Err(std::io::Error::other("the session did not end after quit").into());
    }

    for _ in 0..20 {
        if !Path::new(&socket).exists() {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    Err(std::io::Error::other(format!("the socket was not removed after quit: {socket}")).into())
}
