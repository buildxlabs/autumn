//! The relay's `Range` contract (#179 Q4).

/// A resolved byte range, ready to become a status line and headers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ByteRange {
    /// No usable `Range` header: the whole entity, status 200.
    Full,
    /// A satisfiable range with an inclusive end, status 206.
    Part {
        /// First byte, zero-based.
        start: u64,
        /// Last byte, inclusive.
        end: u64,
    },
    /// An out-of-file range, status 416.
    Unsatisfiable,
}

/// Resolves one `Range` header against an entity of `len` bytes.
///
/// A single `bytes=a-b`, `bytes=a-` or `bytes=-n` is honoured. A multi-range
/// request is answered with its first range; a missing or malformed header is
/// ignored and served as a full 200.
pub(crate) fn resolve(header: Option<&str>, len: u64) -> ByteRange {
    let Some(header) = header else {
        return ByteRange::Full;
    };
    let Some(specs) = header
        .get(..6)
        .filter(|unit| unit.eq_ignore_ascii_case("bytes="))
        .and_then(|_| header.get(6..))
    else {
        return ByteRange::Full;
    };
    let Some(spec) = specs.split(',').next() else {
        return ByteRange::Full;
    };
    if len == 0 {
        return ByteRange::Unsatisfiable;
    }
    let last = len - 1;
    let Some((start_text, end_text)) = spec.trim().split_once('-') else {
        return ByteRange::Full;
    };
    if start_text.is_empty() {
        return match number(end_text) {
            None => ByteRange::Full,
            Some(0) => ByteRange::Unsatisfiable,
            Some(n) => ByteRange::Part {
                start: len.saturating_sub(n),
                end: last,
            },
        };
    }
    let Some(start) = number(start_text) else {
        return ByteRange::Full;
    };
    if end_text.is_empty() {
        return if start >= len {
            ByteRange::Unsatisfiable
        } else {
            ByteRange::Part { start, end: last }
        };
    }
    let Some(end) = number(end_text) else {
        return ByteRange::Full;
    };
    if start > end || start >= len {
        ByteRange::Unsatisfiable
    } else {
        ByteRange::Part {
            start,
            end: end.min(last),
        }
    }
}

/// Parses a plain decimal byte position, rejecting signs and other syntax.
fn number(text: &str) -> Option<u64> {
    if text.is_empty() || !text.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    text.parse::<u64>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_range_serves_the_whole_file() {
        assert_eq!(resolve(None, 10), ByteRange::Full);
        assert_eq!(resolve(Some(""), 10), ByteRange::Full);
        assert_eq!(resolve(Some("bytes=abc"), 10), ByteRange::Full);
    }

    #[test]
    fn a_closed_range_keeps_both_ends() {
        assert_eq!(
            resolve(Some("bytes=2-5"), 10),
            ByteRange::Part { start: 2, end: 5 }
        );
    }

    #[test]
    fn an_open_range_runs_to_the_last_byte() {
        assert_eq!(
            resolve(Some("bytes=7-"), 10),
            ByteRange::Part { start: 7, end: 9 }
        );
    }

    #[test]
    fn a_suffix_range_takes_the_last_n_bytes() {
        assert_eq!(
            resolve(Some("bytes=-3"), 10),
            ByteRange::Part { start: 7, end: 9 }
        );
    }

    #[test]
    fn a_multi_range_is_answered_with_its_first_range() {
        assert_eq!(
            resolve(Some("bytes=0-1,4-5"), 10),
            ByteRange::Part { start: 0, end: 1 }
        );
    }

    #[test]
    fn an_end_past_eof_is_clamped() {
        assert_eq!(
            resolve(Some("bytes=8-100"), 10),
            ByteRange::Part { start: 8, end: 9 }
        );
    }

    #[test]
    fn a_suffix_longer_than_the_file_is_the_whole_file() {
        assert_eq!(
            resolve(Some("bytes=-100"), 10),
            ByteRange::Part { start: 0, end: 9 }
        );
    }

    #[test]
    fn a_start_at_or_past_eof_is_unsatisfiable() {
        assert_eq!(resolve(Some("bytes=10-"), 10), ByteRange::Unsatisfiable);
        assert_eq!(resolve(Some("bytes=10-12"), 10), ByteRange::Unsatisfiable);
    }

    #[test]
    fn an_inverted_range_is_unsatisfiable() {
        assert_eq!(resolve(Some("bytes=5-3"), 10), ByteRange::Unsatisfiable);
    }

    #[test]
    fn an_empty_suffix_is_unsatisfiable() {
        assert_eq!(resolve(Some("bytes=-0"), 10), ByteRange::Unsatisfiable);
    }

    #[test]
    fn an_empty_file_cannot_satisfy_a_range() {
        assert_eq!(resolve(Some("bytes=0-1"), 0), ByteRange::Unsatisfiable);
        assert_eq!(resolve(Some("bytes=-1"), 0), ByteRange::Unsatisfiable);
        assert_eq!(resolve(None, 0), ByteRange::Full);
    }
}
