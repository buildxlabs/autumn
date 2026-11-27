//! v1 infohashes: 40 hex characters, shown lowercase.

use domain::{InfoHash, StreamSource};

const HASH: &str = "0123456789abcdef0123456789abcdef01234567";

#[test]
fn an_infohash_parses_either_case_and_prints_lowercase() {
    let lower: InfoHash = HASH.parse().unwrap();
    let upper: InfoHash = HASH.to_uppercase().parse().unwrap();

    assert_eq!(lower, upper);
    assert_eq!(upper.to_string(), HASH);
    assert_eq!(lower.as_bytes().first(), Some(&0x01));
}

#[test]
fn anything_but_forty_hex_characters_is_rejected() {
    let v2 = "a".repeat(64);
    let signed = "+123456789abcdef0123456789abcdef01234567";
    let short = "123456789abcdef0123456789abcdef01234567";
    for bad in [
        "",
        "abc",
        short,
        "g123456789abcdef0123456789abcdef01234567",
        &v2,
        signed,
    ] {
        assert!(bad.parse::<InfoHash>().is_err(), "{bad:?}");
    }
}

#[test]
fn a_torrent_source_names_one_file_of_a_torrent() {
    let info_hash: InfoHash = HASH.parse().unwrap();

    let source = StreamSource::torrent(info_hash, 3);

    assert_eq!(
        source,
        StreamSource::Torrent {
            info_hash,
            file_index: 3
        }
    );
}
