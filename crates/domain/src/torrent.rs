//! View models for the Torrent List (#207).
//!
//! These are plain, sendable snapshots: the engine samples the embedded
//! session at 4 Hz and publishes them as [`AppEvent::TorrentList`](crate::AppEvent),
//! and the UI store mirrors them without reaching into the session.

use crate::{InfoHash, MediaId};

/// Where a torrent in Autumn's own session came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TorrentOrigin {
    /// Added for a playback stream.
    Stream,
    /// Added to preload the next episode (#208).
    Preload,
    /// Brought in by the user from a magnet or `.torrent` (#181).
    Incoming,
}

/// The lifecycle label of one Torrent List row.
///
/// `Paused` overrides every other state; `Seeding` means complete. There is
/// no separate `complete` or `error` state in Phase 3 (#207 Q3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TorrentState {
    /// Metadata or storage is still initializing; nothing is ready yet.
    Preparing,
    /// Acquiring pieces, with no playback session attached.
    Downloading,
    /// Feeding the one active playback session; pieces may still arrive.
    Streaming,
    /// Progress halted; files are kept. Overrides every other state.
    Paused,
    /// Selected files are complete and uploading while the app runs.
    Seeding,
}

/// The (media, episode) link carried by a torrent, when one is known.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TorrentLink {
    /// Autumn's media identity.
    pub media: MediaId,
    /// The episode number in the media's main run.
    pub episode: u32,
    /// The AniList id, when the media is known by one.
    pub anilist_id: Option<u32>,
}

impl TorrentLink {
    /// Links `episode` of `media`, known on AniList as `anilist_id`.
    #[must_use]
    pub const fn new(media: MediaId, episode: u32, anilist_id: Option<u32>) -> Self {
        Self {
            media,
            episode,
            anilist_id,
        }
    }
}

/// One file of a torrent row, as offered by per-file Play (#207 Q9).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TorrentFileRow {
    /// The index [`StreamSource::Torrent`](crate::StreamSource) selects.
    pub index: usize,
    /// The `/`-separated path inside the torrent.
    pub path: String,
    /// Size in bytes.
    pub len: u64,
    /// Whether every piece of this file is downloaded, so it can be replayed
    /// without waiting on peers.
    pub complete: bool,
}

impl TorrentFileRow {
    /// A file at `index` with its path, size and completion.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "a single completion flag mirrors the file's one boolean fact"
    )]
    #[must_use]
    pub fn new(index: usize, path: impl Into<String>, len: u64, complete: bool) -> Self {
        Self {
            index,
            path: path.into(),
            len,
            complete,
        }
    }
}

/// A point-in-time row of the Torrent List.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TorrentListRow {
    /// The v1 infohash that keys this row.
    pub info_hash: InfoHash,
    /// The display name from metadata or the magnet's `dn`.
    pub name: String,
    /// Where the torrent came from, shown as row info.
    pub origin: TorrentOrigin,
    /// The derived lifecycle label.
    pub state: TorrentState,
    /// Downloaded and selected bytes.
    pub selected_done_bytes: u64,
    /// Total selected bytes.
    pub selected_bytes: u64,
    /// Download speed in bytes per second.
    pub download_bps: u64,
    /// Upload speed in bytes per second.
    pub upload_bps: u64,
    /// Connected peers.
    pub peers: u32,
    /// Uploaded divided by downloaded bytes in thousandths, information only
    /// (#207 Q4). `0` with no downloads; never triggers anything.
    pub ratio_permille: u32,
    /// The (media, episode) link, when known; otherwise Match & play applies.
    pub link: Option<TorrentLink>,
    /// The torrent's files, with completion for per-file Play.
    pub files: Vec<TorrentFileRow>,
    /// A canonical magnet reconstructed from session state (#207 Q10).
    pub magnet: String,
}

/// Derives the row state from live stats and session attachment.
///
/// `paused` wins over everything. A complete torrent seeds unless a playback
/// session is attached; a streaming torrent is `Preparing` until its metadata
/// has arrived, and `Downloading` while it has no pieces yet and no session.
#[expect(
    clippy::fn_params_excessive_bools,
    reason = "the three flags are independent live facts from one stats snapshot"
)]
#[must_use]
pub const fn derive_torrent_state(
    paused: bool,
    finished: bool,
    streaming: bool,
    selected_bytes: u64,
    selected_done_bytes: u64,
) -> TorrentState {
    if paused {
        return TorrentState::Paused;
    }
    if streaming {
        if selected_bytes == 0 {
            return TorrentState::Preparing;
        }
        if selected_done_bytes > 0 {
            return TorrentState::Streaming;
        }
    }
    if finished {
        return TorrentState::Seeding;
    }
    if selected_bytes == 0 {
        return TorrentState::Preparing;
    }
    TorrentState::Downloading
}

/// Counts the rows the rail badge shows: every non-paused torrent, whatever
/// its state (#207 Q12). A paused row is hidden from the count, not removed
/// from the list.
#[must_use]
pub fn active_torrent_count(rows: &[TorrentListRow]) -> u32 {
    rows.iter()
        .filter(|row| row.state != TorrentState::Paused)
        .count()
        .try_into()
        .unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{TorrentState, active_torrent_count, derive_torrent_state};

    #[test]
    fn paused_overrides_streaming_and_seeding() {
        assert_eq!(
            derive_torrent_state(true, true, true, 100, 50),
            TorrentState::Paused,
            "a paused, complete, streaming row is paused"
        );
        assert_eq!(
            derive_torrent_state(true, false, false, 100, 0),
            TorrentState::Paused
        );
    }

    #[test]
    fn complete_rows_seed_unless_streaming() {
        assert_eq!(
            derive_torrent_state(false, true, false, 100, 100),
            TorrentState::Seeding,
            "a preload that finished seeds"
        );
        assert_eq!(
            derive_torrent_state(false, true, true, 100, 100),
            TorrentState::Streaming,
            "a finished file still feeding playback is streaming"
        );
    }

    #[test]
    fn waiting_on_metadata_is_preparing() {
        assert_eq!(
            derive_torrent_state(false, false, false, 0, 0),
            TorrentState::Preparing
        );
        assert_eq!(
            derive_torrent_state(false, false, true, 0, 0),
            TorrentState::Preparing,
            "a stream before metadata arrives is preparing"
        );
    }

    #[test]
    fn acquiring_pieces_without_a_session_is_downloading() {
        assert_eq!(
            derive_torrent_state(false, false, false, 100, 10),
            TorrentState::Downloading,
            "a preload with no pieces yet is still downloading"
        );
        assert_eq!(
            derive_torrent_state(false, false, true, 100, 10),
            TorrentState::Streaming
        );
    }

    /// A minimal row: only the badge-relevant field matters here.
    fn row(state: TorrentState) -> super::TorrentListRow {
        use crate::{InfoHash, TorrentOrigin};

        super::TorrentListRow {
            info_hash: InfoHash::from_bytes([0; 20]),
            name: String::new(),
            origin: TorrentOrigin::Incoming,
            state,
            selected_done_bytes: 0,
            selected_bytes: 0,
            download_bps: 0,
            upload_bps: 0,
            peers: 0,
            ratio_permille: 0,
            link: None,
            files: Vec::new(),
            magnet: String::new(),
        }
    }

    #[test]
    fn the_badge_counts_non_paused_torrents_only() {
        let rows = [
            row(TorrentState::Preparing),
            row(TorrentState::Downloading),
            row(TorrentState::Streaming),
            row(TorrentState::Seeding),
            row(TorrentState::Paused),
            row(TorrentState::Paused),
        ];
        assert_eq!(
            active_torrent_count(&rows),
            4,
            "every live state counts; paused rows do not"
        );
        assert_eq!(
            active_torrent_count(&[]),
            0,
            "an empty list hides the badge"
        );
    }
}
