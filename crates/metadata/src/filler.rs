//! Filler episode lists, scraped from animefillerlist.com.
//!
//! [`AnimeFillerList::filler_episodes`] looks up the anime by its titles
//! (romaji, English and synonyms), parses the filler episode numbers from
//! the show page with `scraper`, and caches the result on disk for 7 days.
//! One request per media, on demand; any failure returns `None` ("unknown",
//! logged at debug) and never fails a scan. See ARCHITECTURE §2 (the
//! `metadata` crate) and decision OD-3.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use scraper::Html;
use tracing::debug;

use net::{Method, Request};

/// How long a fetched filler list stays fresh: 7 days.
const FILLER_TTL: Duration = Duration::from_hours(7 * 24);

/// The site the filler lists live on.
const BASE_URL: &str = "https://www.animefillerlist.com";

/// How far the best matching show title must be from the media's titles.
const MAX_DISTANCE: usize = 10;

/// The Library setting that gates filler lookups (OD-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FillerLookup {
    /// `fetch_filler_episodes` is on: lookups may make requests.
    Enabled,
    /// `fetch_filler_episodes` is off: lookups return "unknown" at once.
    Disabled,
}

/// A filler-list scraper: one request per media, cached on disk for 7 days.
///
/// Requests go through `net`. Cloning shares the HTTP client and the cache
/// directory.
#[derive(Debug, Clone)]
pub struct AnimeFillerList {
    client: net::Client,
    disk: net::PosterCache,
    base_url: String,
    /// The Library setting's `fetch_filler_episodes` value.
    lookup: FillerLookup,
}

impl AnimeFillerList {
    /// A client for the public site, gated by the Library setting. A disabled
    /// setting makes [`Self::filler_episodes`] return `None` without a request.
    #[must_use]
    pub fn new(client: net::Client, disk: net::PosterCache, lookup: FillerLookup) -> Self {
        Self::with_endpoint(client, disk, BASE_URL, lookup)
    }

    /// A client for another endpoint, such as a loopback mock in tests.
    #[must_use]
    pub fn with_endpoint(
        client: net::Client,
        disk: net::PosterCache,
        base_url: impl Into<String>,
        lookup: FillerLookup,
    ) -> Self {
        Self {
            client,
            disk,
            base_url: base_url.into(),
            lookup,
        }
    }

    /// The filler episode numbers of the anime named by `titles` (romaji,
    /// English and synonyms), or `None` when the site or the page cannot be
    /// read: a failure is logged at debug, and the caller leaves the
    /// episodes unmarked.
    pub async fn filler_episodes(&self, titles: &[String]) -> Option<Vec<String>> {
        if self.lookup == FillerLookup::Disabled {
            debug!("filler: disabled by the Library setting");
            return None;
        }
        let mut titles = titles.to_vec();
        titles.sort_unstable();
        titles.dedup();
        let key = format!("filler:titles:{}", titles.join("\u{0}"));
        if let Some(episodes) = self.read_cached(&key).await {
            return Some(episodes);
        }
        let Some(slug) = self.find_show(&titles).await else {
            debug!("filler: no matching show found for the media titles");
            return None;
        };
        let Some(episodes) = self.show_page(&slug).await else {
            debug!(slug, "filler: could not read the show page");
            return None;
        };
        self.store_cached(&key, episodes.clone()).await;
        Some(episodes)
    }

    /// Find the show by its titles: the closest normalised title in the
    /// shows index, preferring exact and containment matches over edits.
    async fn find_show(&self, titles: &[String]) -> Option<String> {
        let body = self.text(&format!("{}/shows", self.base_url)).await?;
        let doc = Html::parse_document(&body);
        let selector = scraper::Selector::parse("div.Group > ul > li > a").ok()?;
        let wanted: Vec<String> = titles.iter().map(|title| normalise(title)).collect();
        let wanted: Vec<String> = wanted
            .into_iter()
            .filter(|title| !title.is_empty())
            .collect();
        let mut best: Option<(usize, String)> = None;
        for entry in doc.select(&selector) {
            let Some(href) = entry.value().attr("href") else {
                continue;
            };
            let shown = normalise(&entry.text().collect::<String>());
            if shown.is_empty() {
                continue;
            }
            let Some((distance, title_len)) = wanted
                .iter()
                .map(|title| (title_score(title, &shown), title.len()))
                .min_by_key(|(distance, _)| *distance)
            else {
                continue;
            };
            let close_enough = distance <= MAX_DISTANCE && distance * 4 <= title_len.max(8);
            if close_enough && best.as_ref().is_none_or(|(best, _)| distance < *best) {
                best = Some((distance, href.trim().to_owned()));
            }
        }
        best.map(|(_, href)| href)
    }

    /// The filler episode numbers of the show page, `None` when it has no
    /// `table.EpisodeList` (a changed layout is "unknown").
    async fn show_page(&self, slug: &str) -> Option<Vec<String>> {
        let base = self.base_url.trim_end_matches('/');
        let slug = slug.trim().trim_start_matches('/');
        let body = self.text(&format!("{base}/{slug}")).await?;
        parse_show(&Html::parse_document(&body))
    }

    /// The response body of `url`, or `None` when it cannot be fetched.
    async fn text(&self, url: &str) -> Option<String> {
        let response = self
            .client
            .read(Request::new(Method::Get, url))
            .await
            .ok()?;
        response.text().await.ok()
    }

    /// The filler list under `key` when it is fresh within [`FILLER_TTL`].
    async fn read_cached(&self, key: &str) -> Option<Vec<String>> {
        let bytes = match self.disk.read_metadata(key).await {
            Ok(bytes) => bytes?,
            Err(error) => {
                debug!(%error, "filler: cache read failed");
                return None;
            }
        };
        let (seconds, episodes): (u64, Vec<String>) = rmp_serde::from_slice(&bytes).ok()?;
        let updated_at = UNIX_EPOCH.checked_add(Duration::from_secs(seconds))?;
        if is_stale(updated_at, FILLER_TTL) {
            return None;
        }
        Some(episodes)
    }

    /// Store the filler list under `key`.
    async fn store_cached(&self, key: &str, episodes: Vec<String>) {
        let seconds = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        if let Ok(bytes) = rmp_serde::to_vec_named(&(seconds, &episodes))
            && let Err(error) = self.disk.write_metadata(key, bytes).await
        {
            debug!(%error, "filler: cache write failed");
        }
    }
}

/// A title lowercased, stripped of its parenthesised alias and reduced to
/// letters, digits and single spaces. The site lists aliases in brackets.
fn normalise(title: &str) -> String {
    let title = title.split(['(', '[']).next().unwrap_or(title);
    let mut out = String::with_capacity(title.len());
    let mut pending_space = false;
    for character in title.chars() {
        if character.is_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            out.extend(character.to_lowercase());
            pending_space = false;
        } else {
            pending_space = true;
        }
    }
    out
}

/// How far `shown` is from `wanted`: edits for different titles, a small
/// penalty for a longer title that simply contains the wanted one.
fn title_score(wanted: &str, shown: &str) -> usize {
    if wanted == shown {
        return 0;
    }
    if wanted.contains(shown) || shown.contains(wanted) {
        let gap = wanted.len().max(shown.len()) - wanted.len().min(shown.len());
        return 1 + gap;
    }
    strsim::levenshtein(wanted, shown)
}

/// Parse a show page: the `td.Number` of every purely `filler` row in
/// `table.EpisodeList`. Mixed canon/filler rows are not filler episodes.
/// `None` when the table is missing or has no filler rows, so a layout change
/// stays "unknown" instead of caching an empty list.
fn parse_show(doc: &Html) -> Option<Vec<String>> {
    let table = scraper::Selector::parse("table.EpisodeList").ok()?;
    doc.select(&table).next()?;
    let rows = scraper::Selector::parse("table.EpisodeList tr").ok()?;
    let number = scraper::Selector::parse("td.Number").ok()?;
    let episodes: Vec<String> = doc
        .select(&rows)
        .filter(|row| row.value().classes().any(|class| class == "filler"))
        .filter_map(|row| {
            row.select(&number)
                .next()
                .map(|cell| cell.text().collect::<String>().trim().to_owned())
        })
        .collect();
    (!episodes.is_empty()).then_some(episodes)
}

/// Whether the stored value is older than `ttl`.
fn is_stale(updated_at: SystemTime, ttl: Duration) -> bool {
    SystemTime::now()
        .duration_since(updated_at)
        .map_or(true, |age| age >= ttl)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use axum::{Router, http::StatusCode, routing::get};
    use scraper::Html;
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    use super::*;

    /// The recorded `/shows` index.
    const INDEX: &str = include_str!("../tests/fixtures/animefillerlist_shows.html");
    /// The recorded show page of a well-known long-running anime.
    const SHOW: &str = include_str!("../tests/fixtures/animefillerlist_naruto.html");

    #[test]
    fn the_recorded_show_page_returns_its_filler_ranges() {
        let episodes = parse_show(&Html::parse_document(SHOW)).unwrap();
        assert_eq!(episodes.len(), 90);
        assert!(episodes.contains(&"101".to_owned()));
        assert!(episodes.contains(&"102".to_owned()));
    }

    #[test]
    fn a_changed_layout_is_unknown() {
        assert!(parse_show(&Html::parse_document("<html></html>")).is_none());
        let no_rows = "<table class=\"EpisodeList\"><tr><td class=\"Number\">1</td></tr></table>";
        assert!(
            parse_show(&Html::parse_document(no_rows)).is_none(),
            "a table without filler rows is a layout change"
        );
    }

    #[test]
    fn titles_are_normalised_before_matching() {
        assert_eq!(
            normalise("Attack on Titan (Shingeki no Kyojin)"),
            "attack on titan"
        );
        assert_eq!(normalise("Naruto Shippuden"), "naruto shippuden");
        assert_eq!(title_score("attack on titan", "attack on titan"), 0);
        assert_eq!(title_score("attack on titan", "attack on titan oads"), 6);
        assert!(title_score("naruto", "naruto shippuden") > 10);
    }

    /// A loopback server serving the recorded index and show page, counting
    /// the requests per path.
    struct Mock {
        filler: AnimeFillerList,
        requests: Arc<std::sync::Mutex<Vec<String>>>,
        _dir: TempDir,
    }

    impl Mock {
        async fn new() -> Self {
            Self::with_pages(StatusCode::OK, INDEX, SHOW, FillerLookup::Enabled).await
        }

        async fn disabled() -> Self {
            Self::with_pages(StatusCode::OK, INDEX, SHOW, FillerLookup::Disabled).await
        }

        async fn changed_layout() -> Self {
            Self::with_pages(
                StatusCode::OK,
                INDEX,
                "<html></html>",
                FillerLookup::Enabled,
            )
            .await
        }

        async fn failing() -> Self {
            // 404 stands for "no page"; net retries server errors but not this.
            Self::with_pages(StatusCode::NOT_FOUND, INDEX, SHOW, FillerLookup::Enabled).await
        }

        async fn with_pages(
            status: StatusCode,
            index: &'static str,
            show: &'static str,
            lookup: FillerLookup,
        ) -> Self {
            let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
            let seen = Arc::clone(&requests);
            let seen_show = Arc::clone(&requests);
            let app = Router::new()
                .route(
                    "/shows",
                    get(move || {
                        let seen = Arc::clone(&seen);
                        async move {
                            seen.lock().unwrap().push("/shows".to_owned());
                            (status, index.to_owned())
                        }
                    }),
                )
                .route(
                    "/shows/naruto",
                    get(move || {
                        let seen = Arc::clone(&seen_show);
                        async move {
                            seen.lock().unwrap().push("/shows/naruto".to_owned());
                            (status, show.to_owned())
                        }
                    }),
                );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let dir = TempDir::new().unwrap();
            Self {
                filler: AnimeFillerList::with_endpoint(
                    net::Client::builder().build().unwrap(),
                    net::PosterCache::new(dir.path()).unwrap(),
                    format!("http://{address}"),
                    lookup,
                ),
                requests,
                _dir: dir,
            }
        }

        fn requests(&self) -> Vec<String> {
            self.requests.lock().unwrap().clone()
        }
    }

    #[tokio::test]
    async fn filler_episodes_caches_within_a_week() {
        let mock = Mock::new().await;
        let titles = vec!["Naruto".to_owned()];

        let first = mock.filler.filler_episodes(&titles).await;
        assert!(first.as_ref().is_some_and(|episodes| !episodes.is_empty()));
        let requests = mock.requests();
        assert_eq!(
            requests,
            vec!["/shows".to_owned(), "/shows/naruto".to_owned()]
        );

        let second = mock.filler.filler_episodes(&titles).await;
        assert_eq!(second, first);
        assert_eq!(
            mock.requests().len(),
            requests.len(),
            "the second call makes no requests"
        );
    }

    #[tokio::test]
    async fn a_changed_layout_returns_unknown_through_the_client() {
        let mock = Mock::changed_layout().await;

        let episodes = mock.filler.filler_episodes(&["Naruto".to_owned()]).await;

        assert_eq!(episodes, None, "a changed page leaves episodes unmarked");
        assert_eq!(
            mock.requests(),
            vec!["/shows".to_owned(), "/shows/naruto".to_owned()]
        );
    }

    #[tokio::test]
    async fn a_network_failure_returns_unknown() {
        let mock = Mock::failing().await;

        let episodes = mock.filler.filler_episodes(&["Naruto".to_owned()]).await;

        assert_eq!(episodes, None);
        assert_eq!(mock.requests(), vec!["/shows".to_owned()]);
    }

    #[tokio::test]
    async fn an_expired_cache_is_refreshed() {
        let mock = Mock::new().await;
        let titles = vec!["Naruto".to_owned()];
        let seconds = (SystemTime::now() - FILLER_TTL - Duration::from_secs(1))
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let key = format!("filler:titles:{}", titles.join("\u{0}"));
        let stale = rmp_serde::to_vec_named(&(seconds, vec!["1".to_owned()])).unwrap();
        mock.filler.disk.write_metadata(&key, stale).await.unwrap();

        let episodes = mock.filler.filler_episodes(&titles).await.unwrap();

        assert_eq!(episodes.len(), 90, "the expired entry was refetched");
        assert_eq!(
            mock.requests(),
            vec!["/shows".to_owned(), "/shows/naruto".to_owned()]
        );
        let stored = mock.filler.disk.read_metadata(&key).await.unwrap().unwrap();
        let (_, stored): (u64, Vec<String>) = rmp_serde::from_slice(&stored).unwrap();
        assert_eq!(stored, episodes, "the fresh list replaced the stale one");
    }

    #[tokio::test]
    async fn a_disabled_library_setting_makes_no_request() {
        let mock = Mock::disabled().await;

        let episodes = mock.filler.filler_episodes(&["Naruto".to_owned()]).await;

        assert_eq!(episodes, None);
        assert!(mock.requests().is_empty(), "the setting gates the requests");
    }

    #[tokio::test]
    async fn titles_resolve_to_the_main_shows_not_their_extras() {
        let mock = Mock::new().await;

        let attack = mock
            .filler
            .find_show(&[
                "Attack on Titan".to_owned(),
                "Shingeki no Kyojin".to_owned(),
            ])
            .await;
        assert_eq!(attack.as_deref(), Some("/shows/attack-titan"));

        let shippuden = mock
            .filler
            .find_show(&["Naruto Shippuden".to_owned()])
            .await;
        assert_eq!(shippuden.as_deref(), Some("/shows/naruto-shippuden"));
    }
}
