// Ported from seanime's `internal/matroska/ebml.go` (see [`File`]'s crate
// docs). Seanime's package is GPL-3.0-only (https://github.com/5rahim/seanime);
// it is itself based on matroska-go (MIT, github.com/luispater/matroska-go).
// Both notices are kept here as AGENTS.md requires.

//! EBML elements and the reader over any seekable source.
//!
//! Ported from seanime's `internal/matroska/ebml.go` (see [`File`]'s crate
//! docs).

use std::io::{Read, Seek, SeekFrom};

use crate::Error;

/// The size above which no element is parsed.
pub(crate) const MAX_ELEMENT_SIZE: u64 = 256 * 1024 * 1024;

/// The indefinite-length size of an element.
const UNKNOWN_SIZE: u64 = (1 << 56) - 1;

/// The EBML header element.
pub(crate) const ID_EBML_HEADER: u64 = 0x1A45_DFA3;
/// `EBMLVersion`.
pub(crate) const ID_EBML_VERSION: u64 = 0x4286;
/// `EBMLReadVersion`.
pub(crate) const ID_EBML_READ_VERSION: u64 = 0x42F7;
/// `EBMLDocType`.
pub(crate) const ID_EBML_DOC_TYPE: u64 = 0x4282;
/// `EBMLDocTypeVersion`.
pub(crate) const ID_EBML_DOC_TYPE_VERSION: u64 = 0x4287;

/// The EBML header of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Header {
    /// The document type, e.g. `matroska`.
    pub(crate) doc_type: String,
    /// The document type version, e.g. 4.
    pub(crate) doc_type_version: u64,
    /// The EBML version.
    pub(crate) version: u64,
    /// The minimum EBML version needed to parse the file.
    pub(crate) read_version: u64,
}

/// A parsed EBML element: ID, size and data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Element {
    pub(crate) id: u64,
    pub(crate) size: u64,
    pub(crate) data: Vec<u8>,
}

impl Element {
    /// The element's data as a big-endian unsigned integer.
    pub(crate) fn uint(&self) -> u64 {
        self.data
            .iter()
            .fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte))
    }

    /// The element's data as a UTF-8 string, without a trailing NUL.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidUtf8`] when the data is not valid UTF-8.
    pub(crate) fn string(&self) -> Result<String, Error> {
        let mut data = self.data.clone();
        if data.last() == Some(&0) {
            data.pop();
        }
        String::from_utf8(data).map_err(Error::InvalidUtf8)
    }

    /// The element's data as a floating-point number. Returns 0 for data of
    /// any other length.
    pub(crate) fn float(&self) -> f64 {
        match self.data.len() {
            4 => f32::from_bits(
                self.data
                    .iter()
                    .take(4)
                    .fold(0u32, |acc, &byte| (acc << 8) | u32::from(byte)),
            )
            .into(),
            8 => f64::from_bits(
                self.data
                    .iter()
                    .take(8)
                    .fold(0u64, |acc, &byte| (acc << 8) | u64::from(byte)),
            ),
            _ => 0.0,
        }
    }
}

/// An EBML reader over any seekable source, tracking the byte position.
#[derive(Debug)]
pub(crate) struct Reader<R: Read + Seek> {
    reader: R,
    pos: u64,
}

impl<R: Read + Seek> Reader<R> {
    pub(crate) fn new(reader: R) -> Self {
        Self { reader, pos: 0 }
    }

    /// The current position of the source.
    pub(crate) fn pos(&self) -> u64 {
        self.pos
    }

    /// Seek the source to an absolute position.
    pub(crate) fn seek_absolute(&mut self, target: u64) -> Result<(), Error> {
        self.reader.seek(SeekFrom::Start(target))?;
        self.pos = target;
        Ok(())
    }

    /// Read an element header: its ID and size.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] on a read failure.
    pub(crate) fn read_element_header(&mut self) -> Result<(u64, u64), Error> {
        let id = self.read_vint_id()?;
        let size = self.read_vint()?;
        Ok((id, size))
    }

    /// Read a complete element: its header and data.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownSize`], [`Error::ElementTooLarge`] or [`Error::Io`]
    /// as in [`Reader::read_element_data`].
    pub(crate) fn read_element(&mut self) -> Result<Element, Error> {
        let (id, size) = self.read_element_header()?;
        let data = self.read_element_data(size)?;
        Ok(Element { id, size, data })
    }

    /// Read `size` bytes of element data.
    ///
    /// # Errors
    ///
    /// [`Error::UnknownSize`] for the indefinite-length size,
    /// [`Error::ElementTooLarge`] above [`MAX_ELEMENT_SIZE`], and
    /// [`Error::Io`] on a read failure.
    pub(crate) fn read_element_data(&mut self, size: u64) -> Result<Vec<u8>, Error> {
        if size == UNKNOWN_SIZE {
            return Err(Error::UnknownSize);
        }
        if size > MAX_ELEMENT_SIZE {
            return Err(Error::ElementTooLarge(size));
        }
        let capacity = usize::try_from(size).map_err(|_| Error::ElementTooLarge(size))?;
        let mut data = vec![0u8; capacity];
        if size > 0 {
            self.read_exact(&mut data)?;
        }
        Ok(data)
    }

    /// Read exactly `buf.len()` bytes.
    pub(crate) fn read_exact(&mut self, buf: &mut [u8]) -> Result<(), Error> {
        self.reader.read_exact(buf)?;
        self.pos += u64::try_from(buf.len()).unwrap_or(u64::MAX);
        Ok(())
    }

    /// Read one byte.
    fn read_byte(&mut self) -> Result<u8, Error> {
        let mut buffer = [0u8; 1];
        self.read_exact(&mut buffer)?;
        Ok(buffer.first().copied().unwrap_or_default())
    }

    /// The first byte of the next VINT and its length: 1 for a byte of
    /// `0x80`, 2 for `0x40`, and so on up to 8 for `0x01`.
    ///
    /// Leading `0x00` bytes are skipped to resync to the next VINT.
    fn read_vint_header(&mut self) -> Result<(u8, u64), Error> {
        let mut first = self.read_byte()?;
        while first == 0x00 {
            first = self.read_byte()?;
        }
        Ok((first, u64::from(first.leading_zeros()) + 1))
    }

    /// Read a variable-length integer, skipping leading zero bytes.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] on a read failure.
    fn read_vint(&mut self) -> Result<u64, Error> {
        let (first, length) = self.read_vint_header()?;
        let mut result = u64::from(first) & ((1_u64 << (8 - length)) - 1);
        for _ in 1..length {
            result = (result << 8) | u64::from(self.read_byte()?);
        }
        Ok(result)
    }

    /// Read a variable-length integer with its length marker kept.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] on a read failure.
    fn read_vint_id(&mut self) -> Result<u64, Error> {
        let (first, length) = self.read_vint_header()?;
        let mut result = u64::from(first);
        for _ in 1..length {
            result = (result << 8) | u64::from(self.read_byte()?);
        }
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A raw byte source over a `Vec` that counts nothing.
    fn source(bytes: &[u8]) -> Reader<std::io::Cursor<&[u8]>> {
        Reader::new(std::io::Cursor::new(bytes))
    }

    #[test]
    fn the_ebml_header_id_reads_back_with_its_marker() {
        let mut reader = source(&[0x1A, 0x45, 0xDF, 0xA3]);
        let id = reader.read_vint_id().unwrap();
        assert_eq!(id, 0x1A45_DFA3);
    }

    #[test]
    fn read_element_reads_id_and_size() {
        // id 0x83 (the 0x80 byte is the length marker), size 0.
        let mut reader = source(&[0x83, 0x80]);
        let element = reader.read_element().unwrap();
        assert_eq!(element.id, 0x83);
        assert_eq!(element.size, 0);
        assert_eq!(element.data.len(), 0);
    }

    #[test]
    fn a_truncated_vint_is_an_error() {
        let mut reader = source(&[]);
        assert!(matches!(reader.read_vint(), Err(Error::Io(_))));
    }

    #[test]
    fn string_strips_a_trailing_nul() {
        let element = Element {
            id: 0,
            size: 3,
            data: b"abc\0".to_vec(),
        };
        assert_eq!(element.string().unwrap(), "abc");
    }

    #[test]
    fn float_reads_a_four_byte_and_eight_byte_value() {
        let short = Element {
            id: 0,
            size: 4,
            data: 1.5f32.to_be_bytes().to_vec(),
        };
        assert!((short.float() - 1.5).abs() < f64::EPSILON);
        let long = Element {
            id: 0,
            size: 8,
            data: 1.5f64.to_be_bytes().to_vec(),
        };
        assert_eq!(long.float(), 1.5);
    }

    #[test]
    fn reading_past_the_end_returns_an_error_not_a_panic() {
        let mut reader = source(&[0x83]);
        // The element header is complete, the data is not: an Io error.
        assert!(matches!(reader.read_element(), Err(Error::Io(_))));
    }
}
