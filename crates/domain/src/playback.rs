//! Playback inputs and the state the engine publishes for the HUD.
//!
//! See ARCHITECTURE §6 and ticket #128. Only a local file is a source in
//! Phase 2; torrent, debrid and online sources land in Phases 3 and 5.

use std::{path::PathBuf, time::Duration};

use crate::MediaId;

/// What the player should open for one episode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamSource {
    /// A local file found by a library scan.
    Local {
        /// The path of the video on disk.
        path: PathBuf,
    },
}

impl StreamSource {
    /// A local file at `path`.
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local { path: path.into() }
    }
}

/// One position update from the active playback session, at most 4 Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PlaybackState {
    /// The media being played.
    pub media: MediaId,
    /// The episode being played.
    pub episode: u32,
    /// Elapsed time from the start of the file.
    pub position: Duration,
    /// The file's duration, when the player knows it.
    pub duration: Option<Duration>,
    /// Whether the player is paused.
    pub paused: bool,
}
