//! Small conversions shared by the indexer parsers.

/// Parse a human-readable size such as `6.6 GiB` or `8423228390 B`.
pub(crate) fn parse_size(value: &str) -> Option<u64> {
    let mut words = value.split_whitespace();
    let amount = words.next()?.replace(',', "").parse::<f64>().ok()?;
    let unit = words.next()?.to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "b" | "byte" | "bytes" => 1.,
        "kb" | "kib" => 1024.,
        "mb" | "mib" => 1024. * 1024.,
        "gb" | "gib" => 1024. * 1024. * 1024.,
        "tb" | "tib" => 1024. * 1024. * 1024. * 1024.,
        _ => return None,
    };
    let bytes = (amount * multiplier).round();
    if bytes < 0. {
        return None;
    }
    format!("{bytes:.0}").parse().ok()
}

/// Parse a count such as `3400` or `3,400`.
pub(crate) fn parse_count(value: &str) -> Option<u32> {
    value.trim().replace(',', "").parse().ok()
}

/// Take the v1 infohash out of a magnet link's `xt=urn:btih:` parameter.
pub(crate) fn infohash_from_magnet(magnet: &str) -> Option<String> {
    let (_, rest) = magnet.split_once("xt=urn:btih:")?;
    let hash = rest.split('&').next()?;
    (!hash.is_empty()).then(|| hash.to_ascii_lowercase())
}

/// A minimal magnet link for an infohash the RSS feed did not link directly.
pub(crate) fn magnet_for(infohash: &str) -> String {
    format!("magnet:?xt=urn:btih:{infohash}")
}

/// Tracker announce URLs from a magnet link's `tr=` parameters, in order.
#[must_use]
pub fn trackers_from_magnet(magnet: &str) -> Vec<String> {
    let Some((_, query)) = magnet.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter_map(|pair| pair.strip_prefix("tr="))
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}
