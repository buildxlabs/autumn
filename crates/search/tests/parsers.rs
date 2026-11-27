//! Parsers over recorded responses; no live network.

use search::animetosho::parse_search;
use search::nyaa::{parse_html, parse_rss};
use url::Url;

const RSS: &str = include_str!("fixtures/nyaa_rss.xml");
const HTML: &str = include_str!("fixtures/nyaa_html.html");
const TOSHO: &str = include_str!("fixtures/animetosho_search.json");

#[test]
fn nyaa_rss_reads_hashes_links_counts_and_flags() {
    let results = parse_rss(RSS).expect("fixture parses");
    assert_eq!(results.len(), 3);

    let first = results.first().expect("first result");
    assert_eq!(first.infohash, "a".repeat(40));
    assert_eq!(
        first.name,
        "[SubsPlease] Sousou no Frieren - 01 (1080p) [ABCD1234].mkv"
    );
    assert_eq!(
        first.torrent_url.as_deref(),
        Some("https://nyaa.si/download/2000001.torrent")
    );
    assert!(
        first
            .magnet_url
            .as_deref()
            .is_some_and(|url| url.contains("xt=urn:btih:aaaaaaaa"))
    );
    assert_eq!(first.size, Some(342_255_206));
    assert_eq!(first.seeders, Some(120));
    assert_eq!(first.leechers, Some(15));
    assert_eq!(first.downloads, Some(3400));
    assert!(first.trusted);
    assert!(!first.remake);

    assert!(
        results.get(1).expect("second result").remake
            && !results.get(1).expect("second result").trusted
    );
    assert!(
        !results.get(2).expect("third result").trusted
            && !results.get(2).expect("third result").remake
    );
}

#[test]
fn nyaa_html_reads_rows_and_resolves_links_against_the_base_url() {
    let base = Url::parse("https://nyaa.si/").expect("valid base");
    let results = parse_html(HTML, &base).expect("fixture parses");
    assert_eq!(results.len(), 2);

    let first = results.first().expect("first result");
    assert_eq!(first.infohash, "a".repeat(40));
    assert_eq!(
        first.torrent_url.as_deref(),
        Some("https://nyaa.si/download/2000001.torrent")
    );
    assert_eq!(first.seeders, Some(120));
    assert_eq!(first.size, Some(342_255_206));
    assert!(first.trusted);
    assert!(!first.remake);

    let second = &results.get(1).expect("second result");
    assert_eq!(second.infohash, "b".repeat(40));
    assert_eq!(second.size, Some(1_288_490_189));
    assert!(second.remake && !second.trusted);
}

#[test]
fn animetosho_json_reads_the_live_feed_shape() {
    let results = parse_search(TOSHO).expect("fixture parses");
    assert_eq!(results.len(), 3);

    let first = results.first().expect("first result");
    assert_eq!(first.infohash, "05bd3a45348e1338117e89cf1e25fd76b7a30de3");
    assert_eq!(
        first.name,
        "[BlackRabbit] Frieren - Beyond Journey's End (2023) - S01 v2 [Bluray-1080p][Opus 2.0][Dual Audio][AV1]"
    );
    assert!(
        first
            .torrent_url
            .as_deref()
            .is_some_and(|url| url.contains("storage.animetosho.net"))
    );
    assert!(
        first
            .magnet_url
            .as_deref()
            .is_some_and(|url| url.starts_with("magnet:?xt=urn:btih:"))
    );
    assert_eq!(first.size, Some(8_423_228_390));
    assert_eq!(first.seeders, Some(46));
    assert_eq!(first.leechers, Some(8));
    assert_eq!(first.downloads, Some(0));
    assert_eq!(first.anidb_aid, Some(17617));
    assert_eq!(first.anidb_eid, None);
    assert_eq!(first.num_files, Some(28));
    assert!(!first.is_seadex);
    assert!(!first.is_batch);
    assert!(!first.trusted && !first.remake);
}

#[test]
fn a_malformed_response_is_an_error_not_an_empty_result() {
    assert!(parse_search("not json").is_err());
    assert_eq!(parse_rss("<rss>").expect("empty feed").len(), 0);
}
