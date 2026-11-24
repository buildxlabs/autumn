//! External player launch and control over JSON IPC.
//!
//! Phase 2 plays video in mpv, IINA, VLC and MPC-HC from a
//! supervised, detached process. This crate owns the launch, the per-launch
//! IPC endpoint and the typed event stream. Detection of installed players
//! and continuity are separate concerns.
//!
//! No GPUI lives here. The public surface is async functions and plain types
//! (ARCHITECTURE §2–3).

mod detect;
mod endpoint;
mod event;
mod iina;
mod ipc;
mod mpv;
#[cfg(test)]
mod testing;

pub use detect::{
    InstalledPlayer, LaunchCommand, Platform, PlayerKind, SearchEnv, detect, preferred,
    validate_custom_path,
};
pub use endpoint::{Endpoint, Retry, fresh_endpoint, sweep_stale};
pub use event::{Chapter, PlayerEvent};
pub use iina::launch_iina;
pub use ipc::Client;
pub use mpv::{LaunchOptions, Session, launch_mpv};

use std::path::PathBuf;

use thiserror::Error;

/// A failure to launch, connect to or drive an external player.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The player executable could not be spawned.
    #[error("failed to spawn player {executable}: {source}")]
    Spawn {
        /// The executable that could not be started.
        executable: String,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },

    /// The per-launch stderr log could not be created.
    #[error("failed to create player log: {source}")]
    Log {
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },

    /// No usable IPC endpoint could be made.
    #[error("could not make an IPC endpoint: {0}")]
    Endpoint(String),

    /// The ipc directory could not be swept.
    #[error("failed to sweep {path}: {source}")]
    Sweep {
        /// The directory that could not be read or cleaned.
        path: PathBuf,
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },

    /// The player never opened its IPC endpoint within the retry budget.
    #[error("player did not open {endpoint} after {attempts} attempts: {source}")]
    Connect {
        /// The endpoint the client tried to reach.
        endpoint: String,
        /// How many connection attempts were made.
        attempts: u32,
        /// The last connection error.
        #[source]
        source: std::io::Error,
    },

    /// The IPC connection failed while reading or writing.
    #[error("player IPC failed: {source}")]
    Ipc {
        /// The operating system's reason.
        #[source]
        source: std::io::Error,
    },

    /// The player rejected a command.
    #[error("player rejected {command}: {message}")]
    Command {
        /// The mpv command that failed.
        command: String,
        /// The player's error string.
        message: String,
    },

    /// The supervisor or the command channel went away.
    #[error("player session is closed")]
    Closed,

    /// A user-entered or detected player path does not exist.
    #[error("no player at {path}")]
    NotFound {
        /// The path that was checked.
        path: PathBuf,
    },

    /// A player path exists but cannot be executed.
    #[error("{path} is not an executable player")]
    NotExecutable {
        /// The path that was checked.
        path: PathBuf,
    },

    /// A player path does not name a supported player.
    #[error("unrecognized player at {path}")]
    Unrecognized {
        /// The path that was checked.
        path: PathBuf,
    },
}

/// The result type used throughout the crate.
pub type Result<T> = std::result::Result<T, Error>;
