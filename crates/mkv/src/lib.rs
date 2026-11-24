//! The Matroska/EBML reader: duration, tracks, chapters and attachments.
//!
//! [`File`] reads only metadata from any `Read + Seek` source (a local file
//! now, a torrent stream reader in `Phase 3`). It follows the `SeekHead` so the
//! tail elements are read without scanning every Cluster, and stops at the
//! first Cluster when there is none. Malformed or truncated input returns an
//! error, never a panic. See ARCHITECTURE §2 (the `mkv` row).
//!
//! Ported from seanime's `internal/matroska` (GPL-3.0, based on MIT
//! `matroska-go`, github.com/luispater/matroska-go, per seanime's
//! `internal/matroska/README.md`).

mod ebml;
mod error;
mod file;

pub use error::Error;
pub use file::{
    Attachment, Chapter, ChapterDisplay, Edition, File, SegmentInfo, TrackInfo, TrackType,
};
