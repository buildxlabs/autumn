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

/// A filler-list scraper: one request per media, cached on disk for 7 days.
///
/// Requests go through `net`. Cloning shares the HTTP client and the cache
/// directory.
#[derive(Debug, Clone)]
pub struct AnimeFillerList {
    client: net::Client,
    disk: net::PosterCache,
    base_url: String,
}

impl AnimeFillerList {
    /// A client for the public site.
    #[must_use]
    pub fn new(client: net::Client, disk: net::PosterCache) -> Self {
        Self::with_endpoint(client, disk, BASE_URL)
    }

    /// A client for another endpoint, such as a loopback mock in tests.
    #[must_use]
    pub fn with_endpoint(
        client: net::Client,
        disk: net::PosterCache,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            client,
            disk,
            base_url: base_url.into(),
        }
    }

    /// The filler episode numbers of the anime named by `titles` (romaji,
    /// English and synonyms), or `None` when the site or the page cannot be
    /// read: a failure is logged at debug, and the caller leaves the
    /// episodes unmarked.
    pub async fn filler_episodes(&self, titles: &[String]) -> Option<Vec<String>> {
        let key = format!("filler:titles:{}", titles.join("\u{0}"));
        if let Some(episodes) = self.read_cached(&key).await {
            return Some(episodes);
        }
        let Some(slug) = self.find_show(titles).await else {
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

    /// Find the show by its titles: the best of the shows index by
    /// Levenshtein distance, when it is within [`MAX_DISTANCE`].
    async fn find_show(&self, titles: &[String]) -> Option<String> {
        let body = self.text(&format!("{}/shows", self.base_url)).await?;
        let doc = Html::parse_document(&body);
        let selector = scraper::Selector::parse("div.Group > ul > li > a").ok()?;
        let mut best: Option<(usize, String)> = None;
        for entry in doc.select(&selector) {
            let Some(slug) = entry.value().attr("href") else {
                continue;
            };
            let title = entry.text().collect::<String>();
            let distance = titles
                .iter()
                .map(|media_title| strsim::levenshtein(media_title, &title))
                .min();
            let Some(distance) = distance else {
                continue;
            };
            if best.as_ref().is_none_or(|(best, _)| distance < *best) {
                best = Some((distance, slug.to_owned()));
            }
        }
        best.and_then(|(distance, slug)| (distance <= MAX_DISTANCE).then_some(slug))
    }

    /// The filler episode numbers of the show page, `None` when it has no
    /// `table.EpisodeList` (a changed layout is "unknown").
    async fn show_page(&self, slug: &str) -> Option<Vec<String>> {
        let body = self.text(&format!("{}{slug}", self.base_url)).await?;
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
        let bytes = self.disk.read_metadata(key).await.ok().flatten()?;
        let (seconds, episodes): (u64, Vec<String>) = rmp_serde::from_slice(&bytes).ok()?;
        let updated_at = UNIX_EPOCH + Duration::from_secs(seconds);
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

/// Parse a show page: the `td.Number` of every `tr` in `table.EpisodeList`
/// whose class list contains `filler`. `None` when there is no such table.
fn parse_show(doc: &Html) -> Option<Vec<String>> {
    let table = scraper::Selector::parse("table.EpisodeList").ok()?;
    doc.select(&table).next()?;
    let rows = scraper::Selector::parse("table.EpisodeList tr").ok()?;
    let number = scraper::Selector::parse("td.Number").ok()?;
    Some(
        doc.select(&rows)
            .filter(|row| row.value().classes().any(|class| class == "filler"))
            .filter_map(|row| {
                row.select(&number)
                    .next()
                    .map(|cell| cell.text().collect::<String>().trim().to_owned())
            })
            .collect(),
    )
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
                            (StatusCode::OK, INDEX.to_owned())
                        }
                    }),
                )
                .route(
                    "/shows/naruto",
                    get(move || {
                        let seen = Arc::clone(&seen_show);
                        async move {
                            seen.lock().unwrap().push("/shows/naruto".to_owned());
                            (StatusCode::OK, SHOW.to_owned())
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
}
