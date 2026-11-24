//! Fresh IPC endpoints and the startup sweep for stale ones.

use std::{fmt, path::Path, time::Duration};

use rand::random;

use crate::{Error, Result};

/// The longest path a Unix domain socket can have on macOS, minus the NUL.
const SOCKET_PATH_MAX: usize = 103;
const SOCKET_PREFIX: &str = "mpv-";
const TEMP_PREFIX: &str = "autumn-mpv-";

/// Where a player listens for IPC commands on one launch.
///
/// On Unix this is a socket path; on Windows it is a named-pipe path. Each
/// launch gets a fresh endpoint so a crash cannot intercept the next session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Endpoint {
    address: String,
}

impl Endpoint {
    /// An HTTP base URL for players without IPC, such as VLC and MPC-HC.
    #[must_use]
    pub fn http(base: impl Into<String>) -> Self {
        Self {
            address: base.into(),
        }
    }

    /// An endpoint at a known address, for transport tests.
    #[cfg(test)]
    pub(crate) fn at(address: impl Into<String>) -> Self {
        Self {
            address: address.into(),
        }
    }

    /// The path or pipe name, for logging and connection.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.address
    }

    /// Connects a client to this endpoint.
    ///
    /// # Errors
    ///
    /// Returns an [`std::io::Error`] if the OS refuses the connection.
    pub(crate) async fn connect(&self) -> std::io::Result<IpcStream> {
        #[cfg(unix)]
        {
            tokio::net::UnixStream::connect(&self.address).await
        }
        #[cfg(windows)]
        {
            tokio::net::windows::named_pipe::ClientOptions::new().open(&self.address)
        }
    }

    /// Removes the endpoint after the session ends.
    ///
    /// Named pipes vanish with the process, so this is a no-op on Windows.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Endpoint`] if a socket file cannot be removed.
    pub fn remove(&self) -> Result<()> {
        #[cfg(unix)]
        {
            match std::fs::remove_file(&self.address) {
                Ok(()) => Ok(()),
                Err(source) if source.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(source) => Err(Error::Endpoint(format!(
                    "could not remove {}: {source}",
                    self.address
                ))),
            }
        }
        #[cfg(windows)]
        {
            let _ = &self.address;
            Ok(())
        }
    }
}

impl fmt::Display for Endpoint {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.address)
    }
}

#[cfg(unix)]
pub(crate) type IpcStream = tokio::net::UnixStream;

#[cfg(windows)]
pub(crate) type IpcStream = tokio::net::windows::named_pipe::NamedPipeClient;

/// The retry budget used while waiting for a player to open its endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Retry {
    /// The maximum number of connect attempts.
    pub attempts: u32,
    /// How long to wait between attempts.
    pub delay: Duration,
}

impl Retry {
    /// A budget of `attempts` tries separated by `delay`.
    pub const fn new(attempts: u32, delay: Duration) -> Self {
        Self { attempts, delay }
    }

    /// The normal budget: 50 tries at 100 ms, five seconds in total.
    pub const fn mpv() -> Self {
        Self::new(50, Duration::from_millis(100))
    }

    /// A longer budget for launchers that return before the player is ready.
    pub const fn iina() -> Self {
        Self::new(100, Duration::from_millis(150))
    }
}

impl Default for Retry {
    fn default() -> Self {
        Self::mpv()
    }
}

/// Makes a fresh endpoint under `cache_dir/ipc`, or a short temp path.
///
/// macOS caps socket paths near 103 bytes. A long cache dir falls back to
/// the OS temp dir. The parent directory is created when needed.
///
/// # Errors
///
/// Returns [`Error::Endpoint`] if the ipc directory cannot be created.
pub fn fresh_endpoint(cache_dir: &Path) -> Result<Endpoint> {
    let suffix = format!("{:016x}", random::<u64>());

    #[cfg(windows)]
    {
        let _ = cache_dir;
        Ok(Endpoint {
            address: format!(r"\\.\pipe\{TEMP_PREFIX}{suffix}"),
        })
    }

    #[cfg(unix)]
    {
        let ipc_dir = cache_dir.join("ipc");
        let name = format!("{SOCKET_PREFIX}{suffix}");
        let preferred = ipc_dir.join(&name);

        let address = if preferred.as_os_str().as_encoded_bytes().len() <= SOCKET_PATH_MAX {
            preferred
        } else {
            std::env::temp_dir().join(format!("{TEMP_PREFIX}{suffix}"))
        };

        if let Some(parent) = address.parent() {
            std::fs::create_dir_all(parent).map_err(|source| {
                Error::Endpoint(format!("could not create {}: {source}", parent.display()))
            })?;
        }
        Ok(Endpoint {
            address: address.to_string_lossy().into_owned(),
        })
    }
}

/// Deletes sockets left by a crashed player.
///
/// Both the cache `ipc/` folder and the temp-dir fallback are swept. Only
/// files with one of our prefixes are removed, and a socket a live session
/// still answers on is left alone. Windows named pipes need no sweep because
/// the OS removes them when the process dies.
///
/// # Errors
///
/// Returns [`Error::Sweep`] if a directory exists but cannot be read or an
/// entry cannot be removed. Missing directories are not an error.
pub fn sweep_stale(cache_dir: &Path) -> Result<()> {
    #[cfg(windows)]
    {
        let _ = cache_dir;
        Ok(())
    }

    #[cfg(unix)]
    {
        sweep_dir(&cache_dir.join("ipc"), SOCKET_PREFIX)?;
        sweep_dir(&std::env::temp_dir(), TEMP_PREFIX)
    }
}

#[cfg(unix)]
fn sweep_dir(dir: &Path, prefix: &str) -> Result<()> {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(source) if source.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(source) => {
            return Err(Error::Sweep {
                path: dir.to_path_buf(),
                source,
            });
        }
    };
    for entry in entries {
        let entry = entry.map_err(|source| Error::Sweep {
            path: dir.to_path_buf(),
            source,
        })?;
        if !entry.file_name().to_string_lossy().starts_with(prefix) {
            continue;
        }
        if std::os::unix::net::UnixStream::connect(entry.path()).is_ok() {
            continue;
        }
        std::fs::remove_file(entry.path()).map_err(|source| Error::Sweep {
            path: entry.path(),
            source,
        })?;
    }
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn two_launches_get_different_endpoints() {
        let cache = tempfile::tempdir().unwrap();
        let a = fresh_endpoint(cache.path()).unwrap();
        let b = fresh_endpoint(cache.path()).unwrap();
        assert_ne!(a, b, "each launch needs its own IPC path");
        assert!(a.as_str().contains(SOCKET_PREFIX));
        assert!(Path::new(a.as_str()).starts_with(cache.path().join("ipc")));
    }

    #[tokio::test]
    async fn connects_to_a_real_unix_socket() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("mpv-fake.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let endpoint = Endpoint {
            address: path.to_string_lossy().into_owned(),
        };

        let server = tokio::spawn(async move { listener.accept().await.is_ok() });
        assert!(endpoint.connect().await.is_ok());
        assert!(server.await.unwrap());
    }

    #[test]
    fn a_long_cache_dir_falls_back_to_the_temp_dir() {
        let long: PathBuf = std::iter::repeat_n("a", 120).collect::<String>().into();
        let cache = Path::new("/").join(long);
        let endpoint = fresh_endpoint(&cache).unwrap();
        assert!(
            endpoint.as_str().len() <= SOCKET_PATH_MAX,
            "socket path must stay under the limit: {}",
            endpoint.as_str()
        );
    }

    #[test]
    fn sweep_removes_only_stale_sockets() {
        let cache = tempfile::tempdir().unwrap();
        let ipc = cache.path().join("ipc");
        std::fs::create_dir_all(&ipc).unwrap();
        let stale = ipc.join("mpv-deadbeef");
        let unrelated = ipc.join("keep-me.txt");
        std::fs::write(&stale, b"").unwrap();
        std::fs::write(&unrelated, b"keep").unwrap();

        sweep_stale(cache.path()).unwrap();

        assert!(!stale.exists(), "the stale socket must be removed");
        assert!(unrelated.exists(), "unrelated files must survive the sweep");
    }

    #[test]
    fn sweep_on_a_missing_directory_is_fine() {
        let cache = tempfile::tempdir().unwrap();
        sweep_stale(&cache.path().join("never-created")).unwrap();
    }

    #[test]
    fn removing_an_absent_endpoint_is_fine() {
        let cache = tempfile::tempdir().unwrap();
        let endpoint = fresh_endpoint(cache.path()).unwrap();
        endpoint.remove().unwrap();
    }
}
