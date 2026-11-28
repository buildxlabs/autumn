//! Validation and mapping of raw provider rows to [`Torrent`]s (#268 Q3–Q4).
//!
//! The host has already checked the top-level array shape. Rows after
//! [`MAX_ROWS`] are truncated; a bad required field drops the row, and a bad
//! optional field only clears that field. Every drop or clear is logged at
//! `debug` with the extension id and path.

use std::sync::Arc;

use domain::InfoHash;
use extensions::ProviderTorrent;
use url::Url;

use crate::provider::ProviderMeta;
use crate::{Error, Indexer, Torrent};

/// At most this many rows are taken from one provider call (#264 Q6).
pub(crate) const MAX_ROWS: usize = 200;

/// Validate one provider call's rows, in order, keeping the survivors.
///
/// # Errors
///
/// Returns [`Error::ProviderInvalidResult`] when rows came back but none
/// survived validation, naming the first row's first required-field error.
pub(crate) fn validate_rows(
    rows: Vec<ProviderTorrent>,
    meta: &ProviderMeta,
) -> Result<Vec<Torrent>, Error> {
    let total = rows.len();
    let mut kept = Vec::new();
    let mut first_error: Option<String> = None;
    for (index, row) in rows.into_iter().enumerate() {
        let path = format!("results[{index}]");
        if index >= MAX_ROWS {
            if index == MAX_ROWS {
                tracing::warn!(extension = %meta.id, %path, "truncated provider rows");
            }
            break;
        }
        match validate_row(row, meta, &path) {
            Ok(torrent) => kept.push(torrent),
            Err(reason) => {
                tracing::debug!(extension = %meta.id, %path, %reason, "dropped provider row");
                if first_error.is_none() {
                    first_error = Some(reason);
                }
            }
        }
    }
    if total > 0 && kept.is_empty() {
        return Err(Error::ProviderInvalidResult {
            id: meta.id.clone(),
            message: first_error.unwrap_or_else(|| "no valid rows".to_owned()),
        });
    }
    Ok(kept)
}

fn validate_row(row: ProviderTorrent, meta: &ProviderMeta, path: &str) -> Result<Torrent, String> {
    let name = row
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .ok_or_else(|| format!("{path}.name: expected a non-empty string"))?
        .to_owned();

    let explicit = row
        .infohash
        .as_deref()
        .and_then(parse_hash)
        .ok_or_else(|| format!("{path}.infohash: not a v1 infohash"));
    // A row without an explicit hash may still carry one in its magnet link.
    let from_magnet = || {
        row.magnet_url
            .as_deref()
            .and_then(magnet_btih)
            .and_then(parse_hash)
    };
    let infohash = match (row.infohash.as_deref(), explicit) {
        (Some(raw), Ok(hash)) if !raw.trim().is_empty() => match from_magnet() {
            Some(other) if other != hash => {
                return Err(format!("{path}.infohash: disagrees with magnet btih"));
            }
            _ => hash,
        },
        (Some(raw), _) if !raw.trim().is_empty() => {
            return Err(format!("{path}.infohash: not a v1 infohash"));
        }
        _ => from_magnet().ok_or_else(|| format!("{path}.infohash: not a v1 infohash"))?,
    };

    let indexer = Indexer::Extension {
        id: meta.id.clone(),
        name: Arc::clone(&meta.name),
    };
    let mut torrent = Torrent::new(indexer, infohash, name);

    if let Some(magnet) = row.magnet_url.as_deref() {
        if magnet.to_ascii_lowercase().starts_with("magnet:") {
            torrent.magnet_url = Some(magnet.to_owned());
        } else {
            clear(meta, path, "magnet_url", "expected a magnet: URI");
        }
    }
    if let Some(link) = row.torrent_url.as_deref() {
        if valid_link(link, meta) {
            torrent.torrent_url = Some(link.to_owned());
        } else {
            clear(meta, path, "torrent_url", "not http(s) on a declared host");
        }
    }

    torrent.size = uint(row.size, 0, u64::MAX, meta, path, "size");
    torrent.published = uint(row.published, 0, u64::MAX, meta, path, "published");
    torrent.seeders = u32_field(row.seeders, 0, meta, path, "seeders");
    torrent.leechers = u32_field(row.leechers, 0, meta, path, "leechers");
    torrent.downloads = u32_field(row.downloads, 0, meta, path, "downloads");
    torrent.anidb_aid = u32_field(row.anidb_aid, 1, meta, path, "anidb_aid");
    torrent.anidb_eid = u32_field(row.anidb_eid, 1, meta, path, "anidb_eid");
    torrent.num_files = u32_field(row.num_files, 0, meta, path, "num_files");
    torrent.is_batch = row.is_batch.unwrap_or(false);
    torrent.files = row.files.unwrap_or_default();
    Ok(torrent)
}

/// A v1 infohash as 40 hex characters or 32 base32 characters, normalized to
/// lowercase hex.
fn parse_hash(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.len() == 40 && raw.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Some(raw.to_ascii_lowercase());
    }
    if raw.len() == 32 {
        return InfoHash::from_base32(raw).ok().map(|hash| hash.to_string());
    }
    None
}

/// The `xt=urn:btih:` value of a magnet link. A `btmh`-only magnet has none.
fn magnet_btih(magnet: &str) -> Option<&str> {
    let query = magnet.strip_prefix("magnet:")?.split_once('?')?.1;
    query
        .split('&')
        .filter_map(|pair| pair.strip_prefix("xt=urn:btih:"))
        .find(|hash| !hash.is_empty())
}

/// A `u32`-valued field, narrowed only after the range check.
fn u32_field(
    value: Option<f64>,
    min: u64,
    meta: &ProviderMeta,
    path: &str,
    field: &str,
) -> Option<u32> {
    uint(value, min, u64::from(u32::MAX), meta, path, field)
        .and_then(|value| u32::try_from(value).ok())
}

/// An integer in `min..=max`, or `None` (with a debug line) when absent or out
/// of range. Luau numbers are doubles, so a non-integer is invalid too.
#[expect(
    clippy::as_conversions,
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss,
    reason = "the value is checked as finite, integral, non-negative and within the bounds before the cast"
)]
fn uint(
    value: Option<f64>,
    min: u64,
    max: u64,
    meta: &ProviderMeta,
    path: &str,
    field: &str,
) -> Option<u64> {
    let value = value?;
    if value.is_finite() && value.fract() == 0. && value >= min as f64 && value <= max as f64 {
        Some(value as u64)
    } else {
        clear(meta, path, field, "expected an integer in range");
        None
    }
}

/// An http(s) link on one of the provider's declared host patterns.
fn valid_link(raw: &str, meta: &ProviderMeta) -> bool {
    let Ok(url) = Url::parse(raw) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https") {
        return false;
    }
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    meta.hosts
        .iter()
        .any(|pattern| host_matches(&host, pattern))
}

fn host_matches(host: &str, pattern: &str) -> bool {
    let pattern = pattern.trim().trim_end_matches('.').to_ascii_lowercase();
    if let Some(suffix) = pattern.strip_prefix("*.") {
        host.strip_suffix(suffix)
            .is_some_and(|label| label.ends_with('.') && label.len() > 1)
    } else {
        host == pattern
    }
}

fn clear(meta: &ProviderMeta, path: &str, field: &str, reason: &str) {
    tracing::debug!(extension = %meta.id, %path, field, reason, "cleared provider field");
}

#[cfg(test)]
mod tests {
    use domain::ExtensionId;

    use super::*;

    type Check = fn(&Torrent) -> bool;

    const HEX: &str = "a94a8fe5ccb19ba61c4c0873d391e987982fbbd3";
    const BASE32: &str = "VFFI7ZOMWGN2MHCMBBZ5HEPJQ6MC7O6T";

    fn meta(hosts: &[&str]) -> ProviderMeta {
        ProviderMeta {
            id: ExtensionId::new("provider"),
            name: Arc::from("Provider"),
            version: "1.0.0".to_owned(),
            hosts: hosts.iter().map(|host| (*host).to_owned()).collect(),
        }
    }

    fn row() -> ProviderTorrent {
        ProviderTorrent {
            name: Some("  [Grp] Show - 05  ".to_owned()),
            infohash: Some(HEX.to_owned()),
            ..ProviderTorrent::default()
        }
    }

    #[test]
    fn valid_rows_normalize_names_and_hashes() {
        let hosts = meta(&["example.org", "*.cdn.example.org"]);
        let cases: Vec<(&str, ProviderTorrent, Check)> = vec![
            ("name is trimmed", row(), |torrent| {
                torrent.name == "[Grp] Show - 05" && torrent.infohash == HEX
            }),
            (
                "base32 becomes lowercase hex",
                ProviderTorrent {
                    infohash: Some(BASE32.to_owned()),
                    ..row()
                },
                |torrent| torrent.infohash == HEX,
            ),
            (
                "magnet btih supplies the hash",
                ProviderTorrent {
                    infohash: None,
                    magnet_url: Some(format!("magnet:?xt=urn:btih:{BASE32}")),
                    ..row()
                },
                |torrent| torrent.infohash == HEX,
            ),
            (
                "trust flags are never mapped",
                ProviderTorrent { ..row() },
                |torrent| !torrent.trusted && !torrent.remake && !torrent.is_seadex,
            ),
        ];

        for (label, input, check) in cases {
            let kept = validate_rows(vec![input], &hosts).unwrap_or_default();
            assert_eq!(kept.len(), 1, "{label}: row should survive");
            assert!(check(kept.first().expect("one row")), "{label}: {kept:?}");
        }
    }

    #[test]
    fn valid_rows_keep_links_and_mapped_optional_fields() {
        let hosts = meta(&["example.org", "*.cdn.example.org"]);
        let cases: Vec<(&str, ProviderTorrent, Check)> = vec![
            (
                "a non-magnet link is cleared",
                ProviderTorrent {
                    magnet_url: Some("https://example.org/x".to_owned()),
                    ..row()
                },
                |torrent| torrent.magnet_url.is_none(),
            ),
            (
                "a declared http host is kept",
                ProviderTorrent {
                    torrent_url: Some("https://example.org/x.torrent".to_owned()),
                    ..row()
                },
                |torrent| torrent.torrent_url.is_some(),
            ),
            (
                "a wildcard host is kept",
                ProviderTorrent {
                    torrent_url: Some("http://a.cdn.example.org/x.torrent".to_owned()),
                    ..row()
                },
                |torrent| torrent.torrent_url.is_some(),
            ),
            (
                "an undeclared host is cleared",
                ProviderTorrent {
                    torrent_url: Some("https://elsewhere.org/x.torrent".to_owned()),
                    ..row()
                },
                |torrent| torrent.torrent_url.is_none(),
            ),
            (
                "a non-http scheme is cleared",
                ProviderTorrent {
                    torrent_url: Some("ftp://example.org/x.torrent".to_owned()),
                    ..row()
                },
                |torrent| torrent.torrent_url.is_none(),
            ),
            (
                "bad optional fields are cleared, the row stays",
                ProviderTorrent {
                    seeders: Some(-1.),
                    leechers: Some(1.5),
                    size: Some(1e20),
                    anidb_aid: Some(0.),
                    published: Some(1_700_000_000.5),
                    num_files: Some(-3.),
                    files: Some(vec!["a.mkv".to_owned()]),
                    ..row()
                },
                |torrent| {
                    torrent.seeders.is_none()
                        && torrent.leechers.is_none()
                        && torrent.size.is_none()
                        && torrent.anidb_aid.is_none()
                        && torrent.published.is_none()
                        && torrent.num_files.is_none()
                        && torrent.files == ["a.mkv"]
                },
            ),
            (
                "valid optional fields map directly",
                ProviderTorrent {
                    size: Some(10_000.),
                    seeders: Some(12.),
                    leechers: Some(3.),
                    downloads: Some(9.),
                    published: Some(1_700_000_000.),
                    is_batch: Some(true),
                    anidb_aid: Some(17_617.),
                    anidb_eid: Some(271_422.),
                    num_files: Some(2.),
                    ..row()
                },
                |torrent| {
                    torrent.size == Some(10_000)
                        && torrent.seeders == Some(12)
                        && torrent.leechers == Some(3)
                        && torrent.downloads == Some(9)
                        && torrent.published == Some(1_700_000_000)
                        && torrent.is_batch
                        && torrent.anidb_aid == Some(17_617)
                        && torrent.anidb_eid == Some(271_422)
                        && torrent.num_files == Some(2)
                },
            ),
        ];

        for (label, input, check) in cases {
            let kept = validate_rows(vec![input], &hosts).unwrap_or_default();
            assert_eq!(kept.len(), 1, "{label}: row should survive");
            assert!(check(kept.first().expect("one row")), "{label}: {kept:?}");
        }
    }

    #[test]
    fn required_field_failures_drop_the_row() {
        let hosts = meta(&["example.org", "*.cdn.example.org"]);
        let invalid: Vec<(&str, ProviderTorrent)> = vec![
            (
                "a btmh-only magnet is invalid",
                ProviderTorrent {
                    infohash: None,
                    magnet_url: Some("magnet:?xt=urn:btmh:1220deadbeef".to_owned()),
                    ..row()
                },
            ),
            (
                "mismatched hashes are invalid",
                ProviderTorrent {
                    magnet_url: Some(format!("magnet:?xt=urn:btih:{}", "b".repeat(40))),
                    ..row()
                },
            ),
            (
                "a blank name is invalid",
                ProviderTorrent {
                    name: Some("   ".to_owned()),
                    ..row()
                },
            ),
            (
                "a bad hash is invalid",
                ProviderTorrent {
                    infohash: Some("not-a-hash".to_owned()),
                    ..row()
                },
            ),
        ];
        for (label, input) in invalid {
            let error = validate_rows(vec![input], &hosts).unwrap_err();
            assert!(
                matches!(error, Error::ProviderInvalidResult { .. }),
                "{label}: {error:?}"
            );
        }
    }

    #[test]
    fn invalid_rows_are_dropped_and_all_invalid_fails_with_the_first_reason() {
        let hosts = meta(&["example.org"]);
        let mut bad_name = row();
        bad_name.name = None;
        let mut bad_hash = row();
        bad_hash.infohash = Some("short".to_owned());
        let mut good = row();
        good.seeders = Some(-1.);

        let kept = validate_rows(vec![bad_name.clone(), good], &hosts).expect("one survives");
        assert_eq!(kept.len(), 1);
        assert!(kept.first().and_then(|torrent| torrent.seeders).is_none());

        let error = validate_rows(vec![bad_name, bad_hash], &hosts).unwrap_err();
        assert!(
            matches!(&error, Error::ProviderInvalidResult { message, .. } if message.contains(".name")),
            "the first row's first error is named: {error:?}"
        );
    }

    #[test]
    fn rows_after_the_cap_are_truncated() {
        let hosts = meta(&[]);
        let rows = (0..=MAX_ROWS)
            .map(|index| ProviderTorrent {
                name: Some(format!("row {index}")),
                ..row()
            })
            .collect();
        let kept = validate_rows(rows, &hosts).expect("the first rows survive");
        assert_eq!(kept.len(), MAX_ROWS);
    }
}
