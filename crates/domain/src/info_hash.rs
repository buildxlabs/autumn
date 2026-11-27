//! The v1 torrent infohash, shared by `search`, `torrent` and playback.

use std::{fmt, str::FromStr};

/// A v1 torrent infohash: the SHA-1 of the info dictionary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct InfoHash([u8; 20]);

/// Why a string is not a v1 infohash.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("an infohash is 40 hexadecimal characters")]
#[non_exhaustive]
pub struct InfoHashError;

impl InfoHash {
    /// Wraps the 20 raw bytes.
    #[must_use]
    pub const fn from_bytes(bytes: [u8; 20]) -> Self {
        Self(bytes)
    }

    /// The 20 raw bytes.
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 20] {
        &self.0
    }
}

impl FromStr for InfoHash {
    type Err = InfoHashError;

    /// Parses 40 hex characters, either case.
    fn from_str(s: &str) -> Result<Self, InfoHashError> {
        if s.len() != 40 {
            return Err(InfoHashError);
        }
        let mut bytes = [0_u8; 20];
        let (pairs, _) = s.as_bytes().as_chunks::<2>();
        for (byte, [high, low]) in bytes.iter_mut().zip(pairs) {
            *byte = nibble(*high)? << 4 | nibble(*low)?;
        }
        Ok(Self(bytes))
    }
}

impl fmt::Display for InfoHash {
    /// Lowercase hex.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
    }
}

/// The value of one hex digit.
fn nibble(digit: u8) -> Result<u8, InfoHashError> {
    char::from(digit)
        .to_digit(16)
        .and_then(|value| u8::try_from(value).ok())
        .ok_or(InfoHashError)
}
