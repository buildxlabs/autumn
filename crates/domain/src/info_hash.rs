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

    /// Parses a 32-character RFC 4648 base32 v1 infohash, either case
    /// (BEP-9 magnet links), normalized to the same 20 bytes.
    ///
    /// # Errors
    ///
    /// [`InfoHashError`] when `s` is not 32 base32 characters of a 20-byte
    /// value.
    pub fn from_base32(s: &str) -> Result<Self, InfoHashError> {
        if s.len() != 32 {
            return Err(InfoHashError);
        }
        let mut bytes = [0_u8; 20];
        let mut bit_buffer = 0_u32;
        let mut bit_count = 0_u32;
        let mut output = 0_usize;
        for &byte in s.as_bytes() {
            let value = base32_digit(byte)?;
            bit_buffer = (bit_buffer << 5) | u32::from(value);
            bit_count += 5;
            if bit_count >= 8 {
                bit_count -= 8;
                let byte =
                    u8::try_from((bit_buffer >> bit_count) & 0xFF).map_err(|_| InfoHashError)?;
                *bytes.get_mut(output).ok_or(InfoHashError)? = byte;
                output += 1;
            }
        }
        // A 32-character group always fills exactly 20 bytes with no remainder.
        if output != 20 {
            return Err(InfoHashError);
        }
        Ok(Self(bytes))
    }
}

/// The value of one RFC 4648 base32 digit, either case.
fn base32_digit(digit: u8) -> Result<u8, InfoHashError> {
    let upper = digit.to_ascii_uppercase();
    match upper {
        b'A'..=b'Z' => Ok(upper - b'A'),
        b'2'..=b'7' => Ok(upper - b'2' + 26),
        _ => Err(InfoHashError),
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

#[cfg(test)]
mod tests {
    use super::InfoHash;

    const HEX: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    const BASE32: &str = "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O6T";

    #[test]
    fn base32_normalizes_to_the_same_bytes_as_hex() {
        let from_hex: InfoHash = HEX.parse().unwrap();
        let from_base32 = InfoHash::from_base32(BASE32).unwrap();
        assert_eq!(from_hex, from_base32);
        assert_eq!(from_base32.to_string(), HEX);
    }

    #[test]
    fn base32_is_case_insensitive() {
        let lower = BASE32.to_ascii_lowercase();
        assert_eq!(InfoHash::from_base32(&lower).unwrap().to_string(), HEX);
    }

    #[test]
    fn malformed_base32_is_rejected() {
        for bad in [
            "",
            "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O6",   // 31 chars
            "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O6TT", // 33 chars
            "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O6!",  // invalid digit
            "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O60",  // '0' not in alphabet
            "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O61",  // '1' not in alphabet
        ] {
            assert!(InfoHash::from_base32(bad).is_err(), "{bad}");
        }
    }
}
