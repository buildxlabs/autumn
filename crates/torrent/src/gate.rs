//! The readiness gate: which pieces of a streamed file must be complete before
//! the player is launched (#179 Q2).

use std::ops::Range;

/// Bytes at the start of the file that must be downloaded: the head window.
pub(crate) const HEAD_WINDOW: u64 = 8 * 1024 * 1024;
/// Bytes at the end of the file that must be downloaded: the tail window.
pub(crate) const TAIL_WINDOW: u64 = 4 * 1024 * 1024;

/// Absolute piece ranges of the head and tail windows of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Windows {
    pub(crate) head: Range<u32>,
    /// Empty when the file is under head + tail: the head then covers it all.
    pub(crate) tail: Range<u32>,
}

/// The byte offset, within the file, where the tail window starts, or `None`
/// when the file is too small to have one.
pub(crate) fn tail_start(file_len: u64) -> Option<u64> {
    (file_len >= HEAD_WINDOW + TAIL_WINDOW).then(|| file_len - TAIL_WINDOW)
}

/// Pieces covering the head and tail windows of a file at `file_offset`
/// (bytes from the start of the torrent), rounded out to whole pieces.
///
/// Returns `None` if a piece index does not fit in `u32` or `piece_len` is 0.
pub(crate) fn windows(file_offset: u64, file_len: u64, piece_len: u32) -> Option<Windows> {
    let piece_len = u64::from(piece_len);
    if piece_len == 0 {
        return None;
    }
    let pieces = |start: u64, end: u64| -> Option<Range<u32>> {
        if start >= end {
            let at = u32::try_from((file_offset + end).div_ceil(piece_len)).ok()?;
            return Some(at..at);
        }
        let first = u32::try_from((file_offset + start) / piece_len).ok()?;
        let last = u32::try_from((file_offset + end).div_ceil(piece_len)).ok()?;
        Some(first..last)
    };
    match tail_start(file_len) {
        Some(tail) => Some(Windows {
            head: pieces(0, HEAD_WINDOW)?,
            tail: pieces(tail, file_len)?,
        }),
        None => Some(Windows {
            head: pieces(0, file_len)?,
            tail: pieces(file_len, file_len)?,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MIB: u64 = 1024 * 1024;
    const PIECE: u32 = 256 * 1024;

    #[test]
    fn large_file_at_torrent_start_takes_8_mib_head_and_4_mib_tail() {
        // 96 MiB at 256 KiB pieces = 384 pieces; 8 MiB = 32 pieces, 4 MiB = 16.
        let w = windows(0, 96 * MIB, PIECE).unwrap();
        assert_eq!(w.head, 0..32, "head pieces");
        assert_eq!(w.tail, 368..384, "tail pieces");
    }

    #[test]
    fn unaligned_file_in_a_batch_rounds_out_to_whole_pieces() {
        // File starts 100 KiB into piece 10 and is 20 MiB + 1 byte long.
        let offset = 10 * u64::from(PIECE) + 100 * 1024;
        let len = 20 * MIB + 1;
        let w = windows(offset, len, PIECE).unwrap();
        // Head ends at offset + 8 MiB = piece 42 + 100 KiB, so piece 42 is partly in.
        assert_eq!(w.head, 10..43, "head pieces");
        // Tail starts at offset + 16 MiB + 1 (piece 74 + 100 KiB + 1),
        // ends at offset + 20 MiB + 1 (inside piece 90).
        assert_eq!(w.tail, 74..91, "tail pieces");
    }

    #[test]
    fn file_under_12_mib_waits_for_the_whole_file() {
        let w = windows(0, 12 * MIB - 1, PIECE).unwrap();
        assert_eq!(w.head, 0..48, "the whole file is the head");
        assert!(w.tail.is_empty(), "no separate tail window");
    }

    #[test]
    fn file_of_exactly_12_mib_has_both_windows() {
        let w = windows(0, 12 * MIB, PIECE).unwrap();
        assert_eq!(w.head, 0..32, "head pieces");
        assert_eq!(w.tail, 32..48, "tail pieces");
    }

    #[test]
    fn zero_piece_length_is_rejected() {
        assert_eq!(windows(0, MIB, 0), None, "no pieces to count");
    }
}
