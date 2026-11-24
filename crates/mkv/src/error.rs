//! Errors returned by [`File::parse`](crate::File::parse) and
//! [`File::attachment_data`](crate::File::attachment_data).

use std::io;

use thiserror::Error;

/// Why a file could not be read as a Matroska file.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The source failed in `std::io`, such as a truncated read.
    #[error(transparent)]
    Io(#[from] io::Error),

    /// The first element was not the EBML header.
    #[error("expected the EBML header, got 0x{0:08X}")]
    NotEbmHeader(u64),

    /// The element after the EBML header was not the segment.
    #[error("expected the segment element, got 0x{0:08X}")]
    NotSegment(u64),

    /// The EBML header names a document type that is not `Matroska` or `WebM`.
    #[error("unsupported document type: {0}")]
    UnsupportedDocType(String),

    /// An element has the indefinite-length size, which is not supported.
    #[error("unknown size elements are not supported")]
    UnknownSize,

    /// A variable-length integer had no length marker.
    #[error("invalid VINT: no length marker")]
    InvalidVint,

    /// An element is larger than the limit for elements and attachments.
    #[error("element size {0} is too large")]
    ElementTooLarge(u64),

    /// An element's data was not valid UTF-8.
    #[error("element data is not valid UTF-8")]
    InvalidUtf8(#[source] std::string::FromUtf8Error),

    /// No attachment is named the one requested.
    #[error("no attachment named {0:?}")]
    AttachmentNotFound(String),
}
