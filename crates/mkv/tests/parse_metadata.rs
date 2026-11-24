//! The `File` reader against the checked-in fixtures.
//!
//! Seams under test:
//! - A test reads the real `fixtures/sample.mkv` and returns its duration,
//!   four tracks, the chapters named `Opening` and `Ending`, and a font
//!   attachment whose data starts with the TrueType magic number.
//! - `fixtures/no_seekhead.mkv`: the reader stops at the first Cluster.
//! - `fixtures/seekhead_gaps.mkv`: the reader follows the `SeekHead` past the
//!   dummy Cluster, and the bytes read do not include it.

#![expect(
    clippy::indexing_slicing,
    reason = "tests index known fixture data; a panic is the failed assertion"
)]

use std::{
    io::{Cursor, Read, Seek, SeekFrom},
    sync::atomic::{AtomicU64, Ordering},
};

use mkv::{File, TrackType};

#[expect(
    clippy::unwrap_used,
    reason = "a missing fixture or a bad path fails the test"
)]
fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(format!(
        "{}/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    ))
    .unwrap()
}

#[test]
fn the_real_fixture_decodes_its_metadata() {
    let mut file = File::parse(Cursor::new(fixture("sample.mkv"))).unwrap();

    let info = file.segment_info().unwrap();
    // 240_000 timecode units at the 1_000_000 ns scale: a 240 s segment.
    assert_eq!(info.duration, Some(240_000_000_000));
    assert_eq!(info.timecode_scale, 1_000_000);

    let tracks = file.tracks();
    assert_eq!(tracks.len(), 4);
    assert!(tracks[0].default);
    assert_eq!(tracks[0].track_type, TrackType::Video);
    assert_eq!(tracks[0].codec_id.as_deref(), Some("V_MPEG4/ISO/AVC"));
    assert_eq!(tracks[1].track_type, TrackType::Audio);
    assert_eq!(tracks[1].codec_id.as_deref(), Some("A_AAC/LC"));
    // 0x88 TrackFlagDefault is absent/zero on both audio tracks: a reader
    // that mistakes 0x55B0 (Colour) for it reports these as default.
    assert!(!tracks[1].default);
    assert_eq!(tracks[2].track_type, TrackType::Audio);
    assert_eq!(tracks[2].codec_id.as_deref(), Some("A_AC3"));
    assert!(!tracks[2].default);
    assert_eq!(tracks[3].track_type, TrackType::Subtitle);
    assert_eq!(tracks[3].codec_id.as_deref(), Some("S_TEXT/ASS"));

    let names: Vec<&str> = file
        .editions()
        .iter()
        .flat_map(|edition| edition.chapters.iter())
        .flat_map(|chapter| chapter.display.iter())
        .map(|display| display.name.as_str())
        .collect();
    assert!(names.contains(&"Opening"), "{names:?}");
    assert!(names.contains(&"Ending"), "{names:?}");

    // Chapter times are nanoseconds, not TimestampScale units.
    let chapters: Vec<_> = file
        .editions()
        .iter()
        .flat_map(|edition| edition.chapters.iter())
        .collect();
    let opening = chapters
        .iter()
        .find(|chapter| chapter.display.iter().any(|d| d.name == "Opening"))
        .unwrap();
    let ending = chapters
        .iter()
        .find(|chapter| chapter.display.iter().any(|d| d.name == "Ending"))
        .unwrap();
    assert_eq!(opening.start, 0);
    assert_eq!(ending.start, 120_000_000_000);

    let font = file.attachment_data("Sample Font.ttf").unwrap();
    let attachment = file
        .attachments()
        .iter()
        .find(|attachment| attachment.name.as_deref() == Some("Sample Font.ttf"))
        .unwrap();
    assert_eq!(attachment.mime_type.as_deref(), Some("font/ttf"));
    assert_eq!(
        attachment.size,
        u64::try_from(font.len()).unwrap_or_default()
    );
    assert!(font.starts_with(&[0x00, 0x01, 0x00, 0x00]));
}

#[test]
fn the_no_seekhead_fixture_stops_at_the_first_cluster() {
    // [header][segment][Info][Tracks][Cluster][Chapters]: the Chapters after
    // the Cluster are not read.
    let file = File::parse(Cursor::new(fixture("no_seekhead.mkv"))).unwrap();
    assert_eq!(file.segment_info().unwrap().duration, Some(240_000_000_000));
    assert_eq!(file.tracks().len(), 4);
    assert_eq!(file.editions().len(), 0);
}

/// Counts the bytes read through the source, so tests can prove where the
/// reader stopped.
#[derive(Debug, Default, Clone)]
struct ReadCounter(std::sync::Arc<AtomicU64>);

impl ReadCounter {
    fn get(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug)]
struct CountingReader {
    inner: Cursor<Vec<u8>>,
    counter: ReadCounter,
}

impl Read for CountingReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let read = self.inner.read(buf)?;
        self.counter
            .0
            .fetch_add(u64::try_from(read).unwrap_or_default(), Ordering::SeqCst);
        Ok(read)
    }
}

impl Seek for CountingReader {
    fn seek(&mut self, pos: SeekFrom) -> std::io::Result<u64> {
        self.inner.seek(pos)
    }
}

#[test]
fn chapters_through_the_seek_head_without_reading_the_clusters() {
    let bytes = fixture("seekhead_gaps.mkv");
    let counter = ReadCounter::default();
    let file = File::parse(CountingReader {
        inner: Cursor::new(bytes.clone()),
        counter: counter.clone(),
    })
    .unwrap();

    let editions = file.editions();
    assert_eq!(editions.len(), 1);
    let names: Vec<&str> = editions[0]
        .chapters
        .iter()
        .flat_map(|chapter| chapter.display.iter())
        .map(|display| display.name.as_str())
        .collect();
    assert!(names.contains(&"Opening"), "{names:?}");
    assert!(names.contains(&"Ending"), "{names:?}");

    let file_len = u64::try_from(bytes.len()).unwrap_or_default();
    assert!(
        counter.get() < file_len - 4096,
        "read {} of {file_len} bytes",
        counter.get()
    );
}
