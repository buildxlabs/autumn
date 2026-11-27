//! Playback inputs and the state the engine publishes for the HUD.
//!
//! See ARCHITECTURE §6 and tickets #128 and #217. Local files and torrent
//! streams are sources; debrid and online sources land in Phase 5.

use std::{path::PathBuf, time::Duration};

use crate::{InfoHash, MediaId};

/// What the player should open for one episode.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum StreamSource {
    /// A local file found by a library scan.
    Local {
        /// The path of the video on disk.
        path: PathBuf,
    },
    /// One file of a torrent, streamed from the embedded session.
    Torrent {
        /// The torrent's v1 infohash.
        info_hash: InfoHash,
        /// The file's index in the torrent's file list.
        file_index: usize,
    },
}

impl StreamSource {
    /// A local file at `path`.
    #[must_use]
    pub fn local(path: impl Into<PathBuf>) -> Self {
        Self::Local { path: path.into() }
    }

    /// File `file_index` of the torrent `info_hash`.
    #[must_use]
    pub const fn torrent(info_hash: InfoHash, file_index: usize) -> Self {
        Self::Torrent {
            info_hash,
            file_index,
        }
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

/// A chapter that can be skipped automatically or from the HUD.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum ChapterKind {
    /// The opening, matched by an opening pattern.
    Opening,
    /// The ending, matched by an ending pattern.
    Ending,
}

/// One chapter of the playing file, as the HUD shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct PlaybackChapter {
    /// The chapter title, when it has one.
    pub title: Option<String>,
    /// Where the chapter starts.
    pub start: Duration,
    /// Where it ends, when known.
    pub end: Option<Duration>,
    /// Whether it matched a skip pattern.
    pub kind: Option<ChapterKind>,
}

impl PlaybackChapter {
    /// A chapter mark with its optional title, bounds and skip kind.
    #[must_use]
    pub fn new(
        title: Option<String>,
        start: Duration,
        end: Option<Duration>,
        kind: Option<ChapterKind>,
    ) -> Self {
        Self {
            title,
            start,
            end,
            kind,
        }
    }
}

/// The countdown to the next library episode, shown after an episode ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct AutoNext {
    /// The media both episodes belong to.
    pub media: MediaId,
    /// The episode that plays when the countdown reaches zero.
    pub episode: u32,
    /// Time left before it plays.
    pub remaining: Duration,
}

impl AutoNext {
    /// A countdown to `episode` of `media` with `remaining` time left.
    #[must_use]
    pub fn new(media: MediaId, episode: u32, remaining: Duration) -> Self {
        Self {
            media,
            episode,
            remaining,
        }
    }
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

/// How far a torrent stream is from launching the player (#179 Q2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum PrepareStage {
    /// Waiting for the torrent's metadata from peers.
    Metadata,
    /// The metadata is known, but no peer is connected.
    Peers,
    /// Downloading the head and tail windows.
    Buffering,
}

/// Downloaded pieces of one readiness window.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct PieceProgress {
    /// Pieces downloaded and verified.
    pub done: u32,
    /// Pieces in the window; zero when the window is empty.
    pub total: u32,
}

impl PieceProgress {
    /// `done` of `total` pieces.
    #[must_use]
    pub const fn new(done: u32, total: u32) -> Self {
        Self { done, total }
    }
}

/// The `Preparing` playback phase of a torrent stream: what the buffer
/// overlay shows before the player opens, at most 4 Hz.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct Preparing {
    /// The media being prepared.
    pub media: MediaId,
    /// The episode being prepared.
    pub episode: u32,
    /// Metadata, peers or buffering.
    pub stage: PrepareStage,
    /// Connected peers.
    pub peers: u32,
    /// Download speed in bytes per second.
    pub download_bps: u64,
    /// Progress of the head window.
    pub head: PieceProgress,
    /// Progress of the tail window.
    pub tail: PieceProgress,
    /// No peer has connected for 60 s: "No peers found. Try another release."
    pub no_peers: bool,
}

impl Preparing {
    /// The first report for `episode` of `media`: waiting for metadata.
    #[must_use]
    pub const fn new(media: MediaId, episode: u32) -> Self {
        Self {
            media,
            episode,
            stage: PrepareStage::Metadata,
            peers: 0,
            download_bps: 0,
            head: PieceProgress::new(0, 0),
            tail: PieceProgress::new(0, 0),
            no_peers: false,
        }
    }
}
