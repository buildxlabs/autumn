//! IINA on macOS, driven through its embedded mpv's JSON IPC.

use std::{path::Path, time::Duration};

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
        |endpoint| iina_args(endpoint, file, options.network_timeout),
        &options,
        ChildPolicy::LeaveOnIpcClose,
    )
}

/// The exact arguments `iina-cli` is launched with, kept pure for testing.
fn iina_args(endpoint: &Endpoint, file: &str, network_timeout: Option<Duration>) -> Vec<String> {
    let mut args = vec![
        "--keep-running".to_owned(),
        "--no-stdin".to_owned(),
        format!("--mpv-input-ipc-server={endpoint}"),
        "--mpv-keep-open=yes".to_owned(),
    ];
    if let Some(timeout) = network_timeout {
        args.push(format!("--mpv-network-timeout={}", timeout.as_secs()));
    }
    args.push(file.to_owned());
    args
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use futures::{StreamExt, channel::mpsc};
    use tokio::time::timeout;

    use super::*;
    use crate::PlayerEvent;

    #[test]
    fn iina_passes_the_stream_network_timeout_to_its_mpv() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = crate::fresh_endpoint(dir.path()).unwrap();
        let args = iina_args(
            &endpoint,
            "http://127.0.0.1:1/t/token/hash/0",
            Some(Duration::from_secs(300)),
        );

        assert!(
            args.contains(&"--mpv-network-timeout=300".to_owned()),
            "{args:?}"
        );
        assert_eq!(
            args.last().map(String::as_str),
            Some("http://127.0.0.1:1/t/token/hash/0")
        );
    }

    #[test]
    fn iina_launch_arguments_forward_the_fresh_ipc_path() {
        let dir = tempfile::tempdir().unwrap();
        let endpoint = crate::fresh_endpoint(dir.path()).unwrap();
        let args = iina_args(&endpoint, "file:///media/ep.mkv", None);

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
