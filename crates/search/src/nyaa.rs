//! `Nyaa` RSS and HTML result parsing.
//!
//! RSS is one page and carries the infohash directly; HTML carries magnet
//! links and row classes for trusted/remake. Both map to the same [`Torrent`].
//! No published numeric rate limit exists; see ADR 0008 and the research.

use std::collections::HashMap;

use quick_xml::{events::Event, reader::Reader};
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::{Error, Indexer, Torrent, util};

/// Parse `Nyaa`'s RSS feed into results, in feed order.
///
/// # Errors
///
/// Returns [`Error::Xml`] if the feed is malformed. Items without a title or
/// an infohash are skipped rather than failing the whole feed.
pub fn parse_rss(xml: &str) -> Result<Vec<Torrent>, Error> {
    let mut reader = Reader::from_str(xml);
    reader.config_mut().trim_text(true);
    let mut fields: HashMap<String, String> = HashMap::new();
    let mut current: Option<String> = None;
    let mut results = Vec::new();

    loop {
        match reader.read_event() {
            Ok(Event::Start(event)) => {
                let name = String::from_utf8_lossy(event.name().as_ref()).into_owned();
                if name == "item" {
                    fields.clear();
                } else {
                    current = Some(name);
                }
            }
            Ok(Event::Text(text)) => {
                if let Some(name) = &current {
                    let raw = std::str::from_utf8(text.as_ref())
                        .map_err(|error| Error::Xml(error.to_string()))?;
                    let value = quick_xml::escape::unescape(raw)
                        .map_err(|error| Error::Xml(error.to_string()))?;
                    fields.insert(name.clone(), value.into_owned());
                }
            }
            Ok(Event::End(event)) => {
                if event.name().as_ref() == b"item"
                    && let Some(torrent) = rss_item(&fields)
                {
                    results.push(torrent);
                }
                current = None;
            }
            Ok(Event::Eof) => break,
            Err(error) => return Err(Error::Xml(error.to_string())),
            _ => {}
        }
    }
    Ok(results)
}

fn rss_item(fields: &HashMap<String, String>) -> Option<Torrent> {
    let infohash = fields.get("nyaa:infoHash")?.trim().to_ascii_lowercase();
    let name = fields.get("title")?.clone();
    let mut torrent = Torrent::new(Indexer::Nyaa, infohash.clone(), name);
    torrent.torrent_url = fields.get("link").cloned();
    torrent.magnet_url = Some(util::magnet_for(&infohash));
    torrent.size = fields
        .get("nyaa:size")
        .and_then(|value| util::parse_size(value));
    torrent.seeders = fields
        .get("nyaa:seeders")
        .and_then(|value| util::parse_count(value));
    torrent.leechers = fields
        .get("nyaa:leechers")
        .and_then(|value| util::parse_count(value));
    torrent.downloads = fields
        .get("nyaa:downloads")
        .and_then(|value| util::parse_count(value));
    torrent.trusted = is_yes(fields.get("nyaa:trusted"));
    torrent.remake = is_yes(fields.get("nyaa:remake"));
    torrent.published = fields.get("pubDate").and_then(|value| parse_rfc2822(value));
    Some(torrent)
}

fn is_yes(value: Option<&String>) -> bool {
    value.is_some_and(|value| value.eq_ignore_ascii_case("yes"))
}

/// Parse one page of `Nyaa`'s HTML listing into results, in row order.
///
/// Links are resolved against `base_url` so a configured mirror yields
/// mirrored links (ADR 0008). Unreadable rows are skipped.
///
/// # Errors
///
/// Returns [`Error::Selector`] only if one of the fixed selectors is malformed.
pub fn parse_html(html: &str, base_url: &Url) -> Result<Vec<Torrent>, Error> {
    let document = Html::parse_document(html);
    let rows = selector("table.torrent-list > tbody > tr")?;
    let title = selector("td:nth-of-type(2) > a:last-of-type")?;
    let links = selector("td:nth-of-type(3) > a")?;
    let size = selector("td:nth-of-type(4)")?;
    let seeds = selector("td:nth-of-type(6)")?;
    let leeches = selector("td:nth-of-type(7)")?;
    let downloads = selector("td:nth-of-type(8)")?;
    let mut results = Vec::new();

    for row in document.select(&rows) {
        let Some(title_link) = row.select(&title).next() else {
            continue;
        };
        let name = attribute(title_link, "title").unwrap_or_else(|| text(title_link));
        let anchors: Vec<ElementRef<'_>> = row.select(&links).collect();
        let torrent_href = anchors
            .first()
            .and_then(|anchor| attribute(*anchor, "href"));
        let magnet = anchors.get(1).and_then(|anchor| attribute(*anchor, "href"));
        let Some(infohash) = magnet.as_deref().and_then(util::infohash_from_magnet) else {
            continue;
        };

        let mut torrent = Torrent::new(Indexer::Nyaa, infohash, name);
        torrent.torrent_url = torrent_href
            .and_then(|href| base_url.join(&href).ok())
            .map(|url| url.to_string());
        torrent.magnet_url = magnet;
        torrent.size = row
            .select(&size)
            .next()
            .map(text)
            .and_then(|value| util::parse_size(&value));
        torrent.seeders = row
            .select(&seeds)
            .next()
            .map(text)
            .and_then(|value| util::parse_count(&value));
        torrent.leechers = row
            .select(&leeches)
            .next()
            .map(text)
            .and_then(|value| util::parse_count(&value));
        torrent.downloads = row
            .select(&downloads)
            .next()
            .map(text)
            .and_then(|value| util::parse_count(&value));
        let classes: Vec<&str> = row.value().classes().collect();
        torrent.trusted = classes.contains(&"success");
        torrent.remake = classes.contains(&"danger");
        results.push(torrent);
    }
    Ok(results)
}

fn selector(pattern: &str) -> Result<Selector, Error> {
    Selector::parse(pattern).map_err(|error| Error::Selector(error.to_string()))
}

fn attribute(element: ElementRef<'_>, name: &str) -> Option<String> {
    element.value().attr(name).map(str::to_owned)
}

fn text(element: ElementRef<'_>) -> String {
    element.text().collect::<String>().trim().to_owned()
}

/// `Nyaa` `pubDate` (RFC 2822) to Unix seconds.
fn parse_rfc2822(value: &str) -> Option<u64> {
    let parsed =
        time::OffsetDateTime::parse(value.trim(), &time::format_description::well_known::Rfc2822)
            .ok()?;
    u64::try_from(parsed.unix_timestamp()).ok()
}
