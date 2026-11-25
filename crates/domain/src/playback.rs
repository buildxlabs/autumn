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

/// What the HUD shows about the episode being played.
///
/// The screen that requests playback knows these labels; the engine only
/// carries them with the request and publishes them to the store. Fallbacks
/// are used when a request has no details.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaybackDetails {
    /// The media's title.
    pub media_title: String,
    /// The episode's title, when it has one.
    pub episode_title: Option<String>,
    /// The media's poster URL, when it has one.
    pub poster: Option<String>,
}

/// Whether to pause or resume the active player.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PlaybackControl {
    /// Pause playback.
    Pause,
    /// Resume playback.
    Resume,
}

impl PlaybackDetails {
    /// Labels for one episode: `media_title`, `episode_title` and `poster`.
    #[must_use]
    pub fn new(
        media_title: impl Into<String>,
        episode_title: Option<String>,
        poster: Option<String>,
    ) -> Self {
        Self {
            media_title: media_title.into(),
            episode_title,
            poster,
        }
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
