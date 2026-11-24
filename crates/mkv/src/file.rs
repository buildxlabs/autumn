// Ported from seanime's `internal/matroska` (GPL-3.0-only,
// https://github.com/5rahim/seanime); the package is itself based on
// matroska-go (MIT, github.com/luispater/matroska-go). Both notices are kept
// as AGENTS.md requires.

//! The Matroska file reader: [`File`], the model types and the parsing.
//!
//! Ported from seanime's `internal/matroska/parser.go` and `types.go`
//! (see the crate docs).

use std::io::{ErrorKind, Read, Seek};

use crate::Error;
use crate::ebml::{
    Element, Header, ID_EBML_DOC_TYPE, ID_EBML_DOC_TYPE_VERSION, ID_EBML_HEADER,
    ID_EBML_READ_VERSION, ID_EBML_VERSION, Reader,
};

/// The segment element.
const ID_SEGMENT: u64 = 0x1853_8067;
/// A `SeekHead` element: the elements the file points to.
const ID_SEEK_HEAD: u64 = 0x114D_9B74;
/// One Seek entry.
const ID_SEEK: u64 = 0x4DBB;
/// The ID of the element a Seek points at.
const ID_SEEK_ID: u64 = 0x53AB;
/// The position of the element a Seek points at, relative to the segment.
const ID_SEEK_POS: u64 = 0x53AC;
/// The `SegmentInfo` element.
const ID_SEGMENT_INFO: u64 = 0x1549_A966;
/// `TimestampScale`, in nanoseconds per timecode unit.
const ID_TIMESTAMP_SCALE: u64 = 0x002A_D7B1;
/// Duration, in timecode units.
const ID_DURATION: u64 = 0x4489;
/// The title of the segment.
const ID_TITLE: u64 = 0x7BA9;
/// `MuxingApp`.
const ID_MUXING_APP: u64 = 0x4D80;
/// `WritingApp`.
const ID_WRITING_APP: u64 = 0x5741;
/// Tracks.
const ID_TRACKS: u64 = 0x1654_AE6B;
/// A `TrackEntry`.
const ID_TRACK_ENTRY: u64 = 0xAE;
/// `TrackNum`.
const ID_TRACK_NUM: u64 = 0xD7;
/// `TrackType`: 1 = video, 2 = audio, 17 = subtitle.
const ID_TRACK_TYPE: u64 = 0x83;
/// `TrackName`.
const ID_TRACK_NAME: u64 = 0x536E;
/// `TrackLanguage`.
const ID_TRACK_LANGUAGE: u64 = 0x0022_B59C;
/// `CodecID`.
const ID_TRACK_CODEC_ID: u64 = 0x86;
/// `TrackFlagDefault`.
const ID_TRACK_FLAG_DEFAULT: u64 = 0x55B0;
/// `TrackFlagForced`.
const ID_TRACK_FLAG_FORCED: u64 = 0x55AA;
/// Cluster: the start of the media data.
const ID_CLUSTER: u64 = 0x1F43_B675;
/// Chapters.
const ID_CHAPTERS: u64 = 0x1043_A770;
/// `EditionEntry`.
const ID_EDITION_ENTRY: u64 = 0x45B9;
/// `EditionUID`.
const ID_EDITION_UID: u64 = 0x45BC;
/// `EditionFlagHidden`.
const ID_EDITION_FLAG_HIDDEN: u64 = 0x45BD;
/// `EditionFlagOrdered`.
const ID_EDITION_FLAG_ORDERED: u64 = 0x45DD;
/// `ChapterAtom`.
const ID_CHAPTER_ATOM: u64 = 0xB6;
/// `ChapterUID`.
const ID_CHAPTER_UID: u64 = 0x73C4;
/// `ChapterTimeStart`.
const ID_CHAPTER_TIME_START: u64 = 0x91;
/// `ChapterTimeEnd`.
const ID_CHAPTER_TIME_END: u64 = 0x92;
/// `ChapterDisplay`.
const ID_CHAPTER_DISPLAY: u64 = 0x80;
/// `ChapterString`.
const ID_CHAPTER_STRING: u64 = 0x85;
/// `ChapterLanguage`.
const ID_CHAPTER_LANGUAGE: u64 = 0x437C;
/// Attachments.
const ID_ATTACHMENTS: u64 = 0x1941_A469;
/// An `AttachedFile`.
const ID_ATTACHED_FILE: u64 = 0x61A7;
/// `FileName`.
const ID_FILE_NAME: u64 = 0x466E;
/// `FileMimeType`.
const ID_FILE_MIME_TYPE: u64 = 0x4660;
/// `FileUID`.
const ID_FILE_UID: u64 = 0x46AE;
/// `FileData`.
const ID_FILE_DATA: u64 = 0x465C;

/// The segment timecode scale in nanoseconds per unit, when the file has none.
const DEFAULT_TIMESCALE: u64 = 1_000_000;

/// A `SeekHead` target: the element ID and the segment-relative position.
type SeekTarget = (u64, u64);

/// The type of a track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum TrackType {
    /// A video track (`1`).
    Video,
    /// An audio track (`2`).
    Audio,
    /// A subtitle track (`17`).
    Subtitle,
    /// Any other value.
    Other(u8),
}

impl TrackType {
    /// The track type of a raw `TrackType` value.
    #[must_use]
    pub fn from_id(id: u64) -> Self {
        match id {
            1 => Self::Video,
            2 => Self::Audio,
            17 => Self::Subtitle,
            other => Self::Other(u8::try_from(other).unwrap_or_default()),
        }
    }
}

/// One track of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrackInfo {
    /// The track number.
    pub number: u8,
    /// The track kind.
    pub track_type: TrackType,
    /// The codec id, such as `V_MPEG4/ISO/AVC`.
    pub codec_id: Option<String>,
    /// The track name.
    pub name: Option<String>,
    /// The language code.
    pub language: Option<String>,
    /// Whether the track is on by default.
    pub default: bool,
    /// Whether the track is forced on.
    pub forced: bool,
}

/// The segment information of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SegmentInfo {
    /// The duration of the segment, in nanoseconds, when the file has one.
    pub duration: Option<u64>,
    /// The segment timecode scale in nanoseconds per timecode unit.
    pub timecode_scale: u64,
    /// The title of the segment.
    pub title: Option<String>,
    /// The application that muxed the file.
    pub muxing_app: Option<String>,
    /// The application that wrote the file.
    pub writing_app: Option<String>,
}

/// One chapter atom.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    /// The chapter UID.
    pub uid: Option<u64>,
    /// The start of the chapter, in the segment's timecode units.
    pub start: u64,
    /// The end of the chapter, in the segment's timecode units.
    pub end: u64,
    /// The display names of the chapter, per language.
    pub display: Vec<ChapterDisplay>,
    /// Nested chapters.
    pub children: Vec<Chapter>,
}

/// One display name of a chapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapterDisplay {
    /// The chapter name.
    pub name: String,
    /// The display language.
    pub language: Option<String>,
}

/// One edition: every edition of a file, with its chapter atoms.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edition {
    /// The edition UID.
    pub uid: Option<u64>,
    /// Whether the edition is hidden.
    pub hidden: bool,
    /// Whether the edition is ordered.
    pub ordered: bool,
    /// The chapter atoms of the edition.
    pub chapters: Vec<Chapter>,
}

/// One attachment of a file, such as a font.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attachment {
    /// The attachment UID.
    pub uid: Option<u64>,
    /// The attachment file name, such as `LiberationSans.ttf`.
    pub name: Option<String>,
    /// The attachment MIME type, such as `font/ttf`.
    pub mime_type: Option<String>,
    /// The size of the attachment data in bytes.
    pub size: u64,
    /// The absolute position of the attachment data in the source.
    pub position: u64,
}

/// A Matroska file's metadata, read from any seekable source.
#[derive(Debug)]
pub struct File<R: Read + Seek> {
    reader: Reader<R>,
    segment_info: Option<SegmentInfo>,
    tracks: Vec<TrackInfo>,
    editions: Vec<Edition>,
    attachments: Vec<Attachment>,
    seek_targets: Vec<SeekTarget>,
}

impl<R: Read + Seek> File<R> {
    /// Read the metadata of the Matroska file on `reader`.
    ///
    /// The reader is only read from, and only what the metadata elements
    /// are needed: no cluster data is read.
    ///
    /// # Errors
    ///
    /// Returns [`Error::NotEbmHeader`] when the first element is not the
    /// EBML header, [`Error::NotSegment`] when the segment is missing, and
    /// [`Error::Io`] or a specific variant when the file is malformed or
    /// truncated.
    pub fn parse(reader: R) -> Result<Self, Error> {
        let mut reader = Reader::new(reader);
        let header = reader.read_element()?;
        if header.id != ID_EBML_HEADER {
            return Err(Error::NotEbmHeader(header.id));
        }
        let header = parse_ebml_header(&header)?;
        if header.doc_type != "matroska" && header.doc_type != "webm" {
            return Err(Error::UnsupportedDocType(header.doc_type));
        }
        let (id, size) = reader.read_element_header()?;
        if id != ID_SEGMENT {
            return Err(Error::NotSegment(id));
        }
        let segment_pos = reader.pos();
        let segment_end = segment_pos.saturating_add(size);
        let mut file = Self {
            reader,
            segment_info: None,
            tracks: Vec::new(),
            editions: Vec::new(),
            attachments: Vec::new(),
            seek_targets: Vec::new(),
        };
        while file.reader.pos() < segment_end {
            let (id, size) = file.reader.read_element_header()?;
            // A Cluster ends the metadata scan; do not read its data.
            if id == ID_CLUSTER {
                break;
            }
            let data = file.reader.read_element_data(size)?;
            let elem = Element { id, size, data };
            let data_base = file.reader.pos().saturating_sub(elem.size);
            match elem.id {
                ID_SEEK_HEAD => file.seek_targets.extend(parse_seek_head(&elem)?),
                ID_SEGMENT_INFO => file.segment_info = Some(parse_segment_info(&elem)?),
                ID_TRACKS => file.tracks.extend(parse_tracks(&elem)?),
                ID_CHAPTERS => file.editions.extend(parse_chapters(&elem)?),
                ID_ATTACHMENTS => file
                    .attachments
                    .extend(parse_attachments(&elem, data_base)?),
                _ => {}
            }
        }
        // Follow the SeekHead for the elements the scan did not reach.
        for (id, seek_pos) in std::mem::take(&mut file.seek_targets) {
            let target = segment_pos.saturating_add(seek_pos);
            match id {
                ID_SEGMENT_INFO if file.segment_info.is_none() => {
                    file.reader.seek_absolute(target)?;
                    let elem = file.reader.read_element()?;
                    file.segment_info = Some(parse_segment_info(&elem)?);
                }
                ID_TRACKS if file.tracks.is_empty() => {
                    file.reader.seek_absolute(target)?;
                    let elem = file.reader.read_element()?;
                    file.tracks.extend(parse_tracks(&elem)?);
                }
                ID_CHAPTERS if file.editions.is_empty() => {
                    file.reader.seek_absolute(target)?;
                    let elem = file.reader.read_element()?;
                    file.editions.extend(parse_chapters(&elem)?);
                }
                ID_ATTACHMENTS if file.attachments.is_empty() => {
                    file.reader.seek_absolute(target)?;
                    let elem = file.reader.read_element()?;
                    let base = file.reader.pos().saturating_sub(elem.size);
                    file.attachments.extend(parse_attachments(&elem, base)?);
                }
                _ => {}
            }
        }
        Ok(file)
    }

    /// The segment information, when the file has any.
    #[must_use]
    pub fn segment_info(&self) -> Option<&SegmentInfo> {
        self.segment_info.as_ref()
    }

    /// The tracks, in track number order.
    #[must_use]
    pub fn tracks(&self) -> &[TrackInfo] {
        &self.tracks
    }

    /// Every edition of the file.
    #[must_use]
    pub fn editions(&self) -> &[Edition] {
        &self.editions
    }

    /// The attachments of the file.
    #[must_use]
    pub fn attachments(&self) -> &[Attachment] {
        &self.attachments
    }

    /// The data of the attachment named `name`, seeking to its position.
    ///
    /// # Errors
    ///
    /// Returns [`Error::AttachmentNotFound`] when no attachment has that
    /// name, and [`Error::Io`] when the data cannot be read.
    pub fn attachment_data(&mut self, name: &str) -> Result<Vec<u8>, Error> {
        let Some(attachment) = self
            .attachments
            .iter()
            .find(|attachment| attachment.name.as_deref() == Some(name))
        else {
            return Err(Error::AttachmentNotFound(name.to_owned()));
        };
        let (position, size) = (attachment.position, attachment.size);
        let capacity = usize::try_from(size).map_err(|_| Error::ElementTooLarge(size))?;
        let mut data = vec![0u8; capacity];
        self.reader.seek_absolute(position)?;
        if size > 0 {
            self.reader.read_exact(&mut data)?;
        }
        Ok(data)
    }
}

/// The reader over the data of an element of a parsed element.
type ChildReader<'a> = Reader<std::io::Cursor<&'a [u8]>>;

/// The reader over the data of `elem`.
fn child_reader(elem: &Element) -> ChildReader<'_> {
    Reader::new(std::io::Cursor::new(elem.data.as_slice()))
}

/// The next element of a child stream, or `None` at its end.
fn next(reader: &mut ChildReader<'_>) -> Result<Option<Element>, Error> {
    match reader.read_element() {
        Ok(elem) => Ok(Some(elem)),
        Err(Error::Io(source)) if source.kind() == ErrorKind::UnexpectedEof => Ok(None),
        Err(error) => Err(error),
    }
}

/// Parse the EBML header element.
fn parse_ebml_header(elem: &Element) -> Result<Header, Error> {
    let mut header = Header {
        doc_type: String::new(),
        doc_type_version: 0,
        version: 0,
        read_version: 0,
    };
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_EBML_VERSION => header.version = e.uint(),
            ID_EBML_READ_VERSION => header.read_version = e.uint(),
            ID_EBML_DOC_TYPE => header.doc_type = e.string()?,
            ID_EBML_DOC_TYPE_VERSION => header.doc_type_version = e.uint(),
            _ => {}
        }
    }
    Ok(header)
}

/// Parse a `SeekHead` element: the `(element ID, segment-relative position)`
/// targets it lists.
fn parse_seek_head(elem: &Element) -> Result<Vec<SeekTarget>, Error> {
    let mut targets = Vec::new();
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        if e.id != ID_SEEK {
            continue;
        }
        let mut id = None;
        let mut pos = 0;
        let mut sub_reader = child_reader(&e);
        while let Some(s) = next(&mut sub_reader)? {
            match s.id {
                ID_SEEK_ID => id = Some(s.uint()),
                ID_SEEK_POS => pos = s.uint(),
                _ => {}
            }
        }
        if let Some(id) = id {
            targets.push((id, pos));
        }
    }
    Ok(targets)
}

/// Parse the `SegmentInfo` element.
fn parse_segment_info(elem: &Element) -> Result<SegmentInfo, Error> {
    let mut info = SegmentInfo {
        duration: None,
        timecode_scale: DEFAULT_TIMESCALE,
        title: None,
        muxing_app: None,
        writing_app: None,
    };
    let mut duration_units: Option<f64> = None;
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_TIMESTAMP_SCALE => info.timecode_scale = e.uint(),
            ID_DURATION => duration_units = Some(e.float()),
            ID_TITLE => info.title = Some(e.string()?),
            ID_MUXING_APP => info.muxing_app = Some(e.string()?),
            ID_WRITING_APP => info.writing_app = Some(e.string()?),
            _ => {}
        }
    }
    let duration = duration_units.and_then(|units| {
        // The EBML spec allows the Duration to be fractional; only whole
        // seconds are used, as with the other integer reads.
        let units = units.trunc();
        if !units.is_finite() || units < 0.0 {
            return None;
        }
        #[expect(
            clippy::as_conversions,
            clippy::cast_possible_truncation,
            clippy::cast_precision_loss,
            clippy::cast_sign_loss,
            reason = "the duration is a whole, non-negative number of seconds"
        )]
        Some((units * info.timecode_scale as f64) as u64)
    });
    info.duration = duration;
    Ok(info)
}

/// Parse the Tracks element.
fn parse_tracks(elem: &Element) -> Result<Vec<TrackInfo>, Error> {
    let mut tracks = Vec::new();
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        if e.id == ID_TRACK_ENTRY {
            tracks.push(parse_track_entry(&e)?);
        }
    }
    tracks.sort_by_key(|track| track.number);
    Ok(tracks)
}

/// Parse one `TrackEntry` element.
fn parse_track_entry(elem: &Element) -> Result<TrackInfo, Error> {
    let mut track = TrackInfo {
        number: 0,
        track_type: TrackType::Other(0),
        codec_id: None,
        name: None,
        language: None,
        default: true,
        forced: false,
    };
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_TRACK_NUM => track.number = u8::try_from(e.uint()).unwrap_or_default(),
            ID_TRACK_TYPE => track.track_type = TrackType::from_id(e.uint()),
            ID_TRACK_NAME => track.name = Some(e.string()?),
            ID_TRACK_LANGUAGE => track.language = Some(e.string()?),
            ID_TRACK_CODEC_ID => track.codec_id = Some(e.string()?),
            ID_TRACK_FLAG_DEFAULT => track.default = e.uint() != 0,
            ID_TRACK_FLAG_FORCED => track.forced = e.uint() != 0,
            _ => {}
        }
    }
    Ok(track)
}

/// Parse the Chapters element.
fn parse_chapters(elem: &Element) -> Result<Vec<Edition>, Error> {
    let mut editions = Vec::new();
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        if e.id == ID_EDITION_ENTRY {
            editions.push(parse_edition_entry(&e)?);
        }
    }
    Ok(editions)
}

/// Parse one `EditionEntry` element.
fn parse_edition_entry(elem: &Element) -> Result<Edition, Error> {
    let mut entry = Edition {
        uid: None,
        hidden: false,
        ordered: false,
        chapters: Vec::new(),
    };
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_EDITION_UID => entry.uid = Some(e.uint()),
            ID_EDITION_FLAG_HIDDEN => entry.hidden = e.uint() != 0,
            ID_EDITION_FLAG_ORDERED => entry.ordered = e.uint() != 0,
            ID_CHAPTER_ATOM => entry.chapters.push(parse_chapter_atom(&e)?),
            _ => {}
        }
    }
    Ok(entry)
}

/// Parse one `ChapterAtom` element.
fn parse_chapter_atom(elem: &Element) -> Result<Chapter, Error> {
    let mut chapter = Chapter {
        uid: None,
        start: 0,
        end: 0,
        display: Vec::new(),
        children: Vec::new(),
    };
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_CHAPTER_UID => chapter.uid = Some(e.uint()),
            ID_CHAPTER_TIME_START => chapter.start = e.uint(),
            ID_CHAPTER_TIME_END => chapter.end = e.uint(),
            ID_CHAPTER_DISPLAY => chapter.display.push(parse_chapter_display(&e)?),
            ID_CHAPTER_ATOM => chapter.children.push(parse_chapter_atom(&e)?),
            _ => {}
        }
    }
    Ok(chapter)
}

/// Parse one `ChapterDisplay` element.
fn parse_chapter_display(elem: &Element) -> Result<ChapterDisplay, Error> {
    let mut display = ChapterDisplay {
        name: String::new(),
        language: None,
    };
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_CHAPTER_STRING => display.name = e.string()?,
            ID_CHAPTER_LANGUAGE => display.language = Some(e.string()?),
            _ => {}
        }
    }
    Ok(display)
}

/// Parse the Attachments element. `base` is the absolute position of the
/// element's data in the source.
fn parse_attachments(elem: &Element, base: u64) -> Result<Vec<Attachment>, Error> {
    let mut attachments = Vec::new();
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        if e.id == ID_ATTACHED_FILE {
            let attached_base = base + reader.pos().saturating_sub(e.size);
            attachments.push(parse_attached_file(&e, attached_base)?);
        }
    }
    Ok(attachments)
}

/// Parse one `AttachedFile` element. `base` is the absolute position of the
/// element's data in the source.
fn parse_attached_file(elem: &Element, base: u64) -> Result<Attachment, Error> {
    let mut attachment = Attachment {
        uid: None,
        name: None,
        mime_type: None,
        size: 0,
        position: base,
    };
    let mut reader = child_reader(elem);
    while let Some(e) = next(&mut reader)? {
        match e.id {
            ID_FILE_NAME => attachment.name = Some(e.string()?),
            ID_FILE_MIME_TYPE => attachment.mime_type = Some(e.string()?),
            ID_FILE_UID => attachment.uid = Some(e.uint()),
            ID_FILE_DATA => {
                attachment.size = e.size;
                attachment.position = base + reader.pos().saturating_sub(e.size);
            }
            _ => {}
        }
    }
    Ok(attachment)
}

#[cfg(test)]
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::indexing_slicing,
    clippy::needless_pass_by_value,
    reason = "tests index their own writer's data and compare whole numbers"
)]
mod tests {
    use super::*;
    use std::io::Cursor;

    // A small EBML writer, the same structure the fixture generator uses.

    fn vint(value: u64) -> Vec<u8> {
        let mut length = 1_u64;
        while value >= (1_u64 << (7 * length)) && length < 8 {
            length += 1;
        }
        let length = u8::try_from(length).unwrap_or(8);
        if length == 8 {
            let mut bytes = vec![0x00];
            bytes.extend_from_slice(&value.to_be_bytes()[1..]);
            return bytes;
        }
        let marker = 1_u64 << (8 - length);
        let shift = 8 * (u64::from(length) - 1);
        let first_bits = (value >> shift) & (marker - 1);
        let mut bytes = vec![(marker | first_bits) as u8];
        if length > 1 {
            let rest = value & ((1_u64 << shift) - 1);
            let rest_bytes = rest.to_be_bytes();
            let start = 8 - (length - 1) as usize;
            bytes.extend_from_slice(&rest_bytes[start..]);
        }
        bytes
    }

    fn elem(id: u64, data: Vec<u8>) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&u32::try_from(id).unwrap_or_default().to_be_bytes());
        out.extend_from_slice(&vint(u64::try_from(data.len()).unwrap_or_default()));
        out.extend_from_slice(&data);
        out
    }

    fn cat(parts: &[&[u8]]) -> Vec<u8> {
        parts.iter().flat_map(|part| part.iter().copied()).collect()
    }

    fn uelem(id: u64, value: u64) -> Vec<u8> {
        elem(id, value.to_be_bytes().to_vec())
    }

    fn str_elem(id: u64, text: &str) -> Vec<u8> {
        elem(id, text.as_bytes().to_vec())
    }

    fn f64_elem(id: u64, value: f64) -> Vec<u8> {
        elem(id, value.to_be_bytes().to_vec())
    }

    fn chapter_atom(name: &str, start: u64) -> Vec<u8> {
        elem(
            ID_CHAPTER_ATOM,
            cat(&[
                &uelem(ID_CHAPTER_TIME_START, start),
                &elem(
                    ID_CHAPTER_DISPLAY,
                    cat(&[
                        &str_elem(ID_CHAPTER_STRING, name),
                        &str_elem(ID_CHAPTER_LANGUAGE, "eng"),
                    ]),
                ),
            ]),
        )
    }

    /// [EBML header][Segment][Info][Tracks][Chapters].
    fn sample_stream() -> Vec<u8> {
        let info = elem(
            ID_SEGMENT_INFO,
            cat(&[
                &uelem(ID_TIMESTAMP_SCALE, 1_000_000),
                &f64_elem(ID_DURATION, 240_000.0),
                &str_elem(ID_TITLE, "Sample Episode"),
            ]),
        );
        let tracks = elem(
            ID_TRACKS,
            cat(&[
                &elem(
                    ID_TRACK_ENTRY,
                    cat(&[
                        &uelem(ID_TRACK_NUM, 1),
                        &uelem(ID_TRACK_TYPE, 2),
                        &str_elem(ID_TRACK_CODEC_ID, "A_AAC/LC"),
                    ]),
                ),
                &elem(
                    ID_TRACK_ENTRY,
                    cat(&[
                        &uelem(ID_TRACK_NUM, 2),
                        &uelem(ID_TRACK_TYPE, 17),
                        &str_elem(ID_TRACK_CODEC_ID, "S_TEXT/ASS"),
                    ]),
                ),
            ]),
        );
        let chapters = elem(
            ID_CHAPTERS,
            elem(ID_EDITION_ENTRY, chapter_atom("Opening", 0)),
        );
        let header = elem(
            ID_EBML_HEADER,
            cat(&[
                &uelem(ID_EBML_VERSION, 1),
                &str_elem(ID_EBML_DOC_TYPE, "matroska"),
                &uelem(ID_EBML_DOC_TYPE_VERSION, 2),
            ]),
        );
        let mut stream = header;
        stream.extend(elem(ID_SEGMENT, cat(&[&info, &tracks, &chapters])));
        stream
    }

    #[test]
    fn the_sample_stream_parses_its_metadata() {
        let file = File::parse(Cursor::new(sample_stream())).unwrap();
        let info = file.segment_info().unwrap();
        assert_eq!(info.duration, Some(240_000_000_000));
        assert_eq!(info.title.as_deref(), Some("Sample Episode"));
        assert_eq!(file.tracks().len(), 2);
        assert_eq!(file.tracks()[0].track_type, TrackType::Audio);
        assert_eq!(file.tracks()[0].codec_id.as_deref(), Some("A_AAC/LC"));
        assert_eq!(file.tracks()[1].track_type, TrackType::Subtitle);
        assert_eq!(file.editions()[0].chapters[0].display[0].name, "Opening");
    }

    #[test]
    fn chapters_after_the_first_cluster_are_read_through_the_seek_head() {
        // [EBML header][Segment][SeekHead{Chapters}, Cluster, Chapters]:
        // the scan stops at the Cluster and the SeekHead target is followed.
        let chapters = elem(
            ID_CHAPTERS,
            elem(ID_EDITION_ENTRY, chapter_atom("Opening", 0)),
        );
        let info = elem(ID_SEGMENT_INFO, uelem(ID_TIMESTAMP_SCALE, 1_000_000));
        let cluster = elem(ID_CLUSTER, uelem(0xE7, 0));
        let mut target = 0u64;
        for _ in 0..3 {
            let seek_head = elem(
                ID_SEEK_HEAD,
                elem(
                    ID_SEEK,
                    cat(&[
                        &elem(
                            ID_SEEK_ID,
                            u32::try_from(ID_CHAPTERS)
                                .unwrap_or_default()
                                .to_be_bytes()
                                .to_vec(),
                        ),
                        &uelem(ID_SEEK_POS, target),
                    ]),
                ),
            );
            let next =
                u64::try_from(seek_head.len() + info.len() + cluster.len()).unwrap_or_default();
            if next == target {
                break;
            }
            target = next;
        }
        let header = elem(
            ID_EBML_HEADER,
            cat(&[
                &uelem(ID_EBML_VERSION, 1),
                &str_elem(ID_EBML_DOC_TYPE, "matroska"),
            ]),
        );
        let mut stream = header;
        stream.extend(elem(
            ID_SEGMENT,
            cat(&[
                &elem(
                    ID_SEEK_HEAD,
                    elem(
                        ID_SEEK,
                        cat(&[
                            &elem(
                                ID_SEEK_ID,
                                u32::try_from(ID_CHAPTERS)
                                    .unwrap_or_default()
                                    .to_be_bytes()
                                    .to_vec(),
                            ),
                            &uelem(ID_SEEK_POS, target),
                        ]),
                    ),
                ),
                &info,
                &cluster,
                &chapters,
            ]),
        ));

        let file = File::parse(Cursor::new(stream)).unwrap();
        let editions = file.editions();
        assert_eq!(editions.len(), 1);
        assert_eq!(editions[0].chapters.len(), 1);
        assert_eq!(editions[0].chapters[0].display[0].name, "Opening");
    }

    #[test]
    fn truncated_input_is_an_error() {
        let stream = sample_stream();
        assert!(File::parse(Cursor::new(stream.clone())).is_ok());
        for len in [0usize, 1, 3, 7, stream.len() / 2, stream.len() - 1] {
            let prefix: Vec<u8> = stream.iter().take(len).copied().collect();
            assert!(
                File::parse(Cursor::new(prefix)).is_err(),
                "a {len}-byte prefix is truncated"
            );
        }
    }

    #[test]
    fn random_garbage_is_an_error() {
        let mut state = 0x2545_F491_4F6C_DD1D_u64;
        for _ in 0..32 {
            let len = 1 + usize::try_from(state % 64).unwrap_or_default();
            let mut bytes = Vec::new();
            for _ in 0..len {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1);
                bytes.push(u8::try_from((state >> 56) & 0xFF).unwrap_or_default());
            }
            assert!(
                File::parse(Cursor::new(bytes)).is_err(),
                "garbage is rejected"
            );
        }
    }
}
