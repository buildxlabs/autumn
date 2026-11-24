//! IINA on macOS, driven through its embedded mpv's JSON IPC.

use std::path::Path;

use crate::{
    Endpoint, LaunchOptions, Result, Retry, Session,
    mpv::{self, ChildPolicy},
};

/// Launches IINA on `file` and returns its supervised session.
///
/// `iina-cli` hands the file to the app and returns before mpv is ready, so
/// [`LaunchOptions::iina`] uses the longer retry budget. Closing the IINA
/// window can leave the app running: an IPC close ends the session without
/// killing the process.
///
/// # Errors
///
/// Returns an error if the endpoint, log or process cannot be created.
pub fn launch_iina(
    executable: impl AsRef<Path>,
    file: &str,
    options: &LaunchOptions,
) -> Result<Session> {
    // iina-cli returns before the app opens IPC, so the longer budget is
    // required, not something a caller has to remember to opt into.
    let options = LaunchOptions {
        ipc_retry: Retry::iina(),
        ..options.clone()
    };
    mpv::launch(
        executable,
        |endpoint| iina_args(endpoint, file),
        &options,
        ChildPolicy::LeaveOnIpcClose,
    )
}

/// The exact arguments `iina-cli` is launched with, kept pure for testing.
fn iina_args(endpoint: &Endpoint, file: &str) -> Vec<String> {
    vec![
        "--keep-running".to_owned(),
        "--no-stdin".to_owned(),
        format!("--mpv-input-ipc-server={endpoint}"),
        "--mpv-keep-open=yes".to_owned(),
        file.to_owned(),
    ]
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{StreamExt, channel::mpsc};
    use tokio::time::timeout;

    use super::*;
    use crate::PlayerEvent;

    #[test]
    fn iina_launch_arguments_forward_the_fresh_ipc_path() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = crate::fresh_endpoint(dir.path()).unwrap();
        let args = iina_args(&endpoint, "file:///media/ep.mkv");

        assert_eq!(
            args,
            vec![
                "--keep-running",
                "--no-stdin",
                &format!("--mpv-input-ipc-server={endpoint}"),
                "--mpv-keep-open=yes",
                "file:///media/ep.mkv",
            ]
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ipc_close_ends_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = crate::Endpoint::at(dir.path().join("iina.sock").to_string_lossy());
        let (client_stream, server_stream) = tokio::io::duplex(64 * 1024);
        let mut fake = crate::testing::FakeMpv::from_stream(server_stream);
        let (_command_tx, command_rx) = mpsc::unbounded();
        let (event_tx, mut event_rx) = mpsc::channel(8);
        let child = tokio::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .unwrap();

        let supervisor = tokio::spawn(mpv::supervise_connected(
            client_stream,
            child,
            endpoint,
            ChildPolicy::LeaveOnIpcClose,
            event_tx,
            command_rx,
        ));
        // Let the pump finish its observations, then close the transport for
        // real so the test exercises EOF rather than a failed write.
        fake.drain_observations().await;
        drop(fake);

        let event = timeout(Duration::from_secs(2), event_rx.next())
            .await
            .expect("the session must end when IPC closes");
        assert_eq!(event, Some(PlayerEvent::Exited));
        let mut child = timeout(Duration::from_secs(2), supervisor)
            .await
            .expect("the supervisor must finish")
            .expect("the supervisor task must not panic");

        assert!(
            child.try_wait().ok().flatten().is_none(),
            "IINA must be left running when IPC closes"
        );
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}
