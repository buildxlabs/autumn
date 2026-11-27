//! Search orchestration: URL construction, budget, retry, cache and dedupe.
//!
//! Each indexer runs its own serial queue and the two queues run concurrently.
//! Numbered queries come first; keyword fallbacks only run when that indexer
//! found nothing. Results are cached for 60 seconds and deduped by infohash.

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    sync::{
        Arc, Mutex, MutexGuard,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use url::Url;

use crate::{Error, Indexer, Query, QueryPlan, SearchRequest, Torrent, animetosho, nyaa, plan};

/// `Nyaa`'s default base URL; a mirror can be supplied in [`Config`].
pub const NYAA_BASE_URL: &str = "https://nyaa.si";
/// The live `AnimeTosho` successor feed, not the retired `.org` host.
pub const ANIMETOSHO_BASE_URL: &str = "https://feed.animetosho.net";
/// How long a query's results stay usable.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
const DEFAULT_BACKOFF: Duration = Duration::from_millis(250);

/// Base URLs for the two indexers, supplied by Network settings later.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Config {
    /// `Nyaa` base URL; a bare host is normalized to `https://`.
    pub nyaa_base_url: String,
    /// `AnimeTosho` base URL; the retired `feed.animetosho.org` is not used.
    pub animetosho_base_url: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            nyaa_base_url: NYAA_BASE_URL.to_owned(),
            animetosho_base_url: ANIMETOSHO_BASE_URL.to_owned(),
        }
    }
}

impl Config {
    /// Override the `Nyaa` base URL with a mirror from Network settings.
    #[must_use]
    pub fn with_nyaa_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.nyaa_base_url = base_url.into();
        self
    }

    /// Override the `AnimeTosho` base URL from Network settings.
    #[must_use]
    pub fn with_animetosho_base_url(mut self, base_url: impl Into<String>) -> Self {
        self.animetosho_base_url = base_url.into();
        self
    }
}

/// Where response bytes come from. Tests inject recorded fixtures here.
pub trait Transport: Send + Sync {
    /// Fetch one query's single page of results.
    ///
    /// # Errors
    ///
    /// Returns a transport, HTTP or rate-limit error; the searcher retries
    /// retryable failures once.
    fn fetch(&self, query: &Query) -> impl Future<Output = Result<Vec<u8>, Error>> + Send;
}

/// A clock abstraction so the 60-second cache can be tested without waiting.
pub trait Clock: Send + Sync {
    /// The current instant.
    fn now(&self) -> Instant;
}

/// The wall clock used in production.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> Instant {
        Instant::now()
    }
}

/// A clock advanced explicitly by tests.
#[derive(Debug, Clone)]
pub struct FakeClock {
    start: Instant,
    offset_ms: Arc<AtomicU64>,
}

impl Default for FakeClock {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            offset_ms: Arc::new(AtomicU64::new(0)),
        }
    }
}

impl FakeClock {
    /// Move the clock forward, expiring cached results.
    pub fn advance(&self, duration: Duration) {
        let millis = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
        self.offset_ms.fetch_add(millis, Ordering::Relaxed);
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Instant {
        let millis = self.offset_ms.load(Ordering::Relaxed);
        self.start + Duration::from_millis(millis)
    }
}

#[derive(Debug, Clone)]
struct CacheEntry {
    at: Instant,
    results: Vec<Torrent>,
}

/// Runs queries against both indexers within the #181 budget.
#[derive(Debug)]
pub struct Searcher<T: Transport, C: Clock = SystemClock> {
    transport: T,
    cache: Mutex<HashMap<Query, CacheEntry>>,
    clock: C,
}

impl<T: Transport> Searcher<T, SystemClock> {
    /// Build a searcher using the wall clock.
    #[must_use]
    pub fn new(transport: T) -> Self {
        Self::with_clock(transport, SystemClock)
    }
}

impl<T: Transport, C: Clock> Searcher<T, C> {
    /// Build a searcher with an injected clock.
    #[must_use]
    pub fn with_clock(transport: T, clock: C) -> Self {
        Self {
            transport,
            cache: Mutex::new(HashMap::new()),
            clock,
        }
    }

    /// Search both indexers concurrently and return deduped results.
    ///
    /// # Errors
    ///
    /// Returns an error if either indexer fails after its one retry, or if a
    /// response cannot be parsed.
    pub async fn search(&self, request: &SearchRequest) -> Result<Vec<Torrent>, Error> {
        let queries = plan(request);
        let (nyaa, tosho) = tokio::join!(
            self.run_indexer(&queries, Indexer::Nyaa),
            self.run_indexer(&queries, Indexer::AnimeTosho),
        );
        let mut results = nyaa?;
        results.extend(tosho?);
        let mut seen = HashSet::new();
        results.retain(|torrent| seen.insert(torrent.infohash.to_ascii_lowercase()));
        Ok(results)
    }

    async fn run_indexer(
        &self,
        queries: &QueryPlan,
        indexer: Indexer,
    ) -> Result<Vec<Torrent>, Error> {
        let mut results = Vec::new();
        for query in queries
            .numbered
            .iter()
            .filter(|query| query.indexer() == indexer)
        {
            results.extend(self.run_query(query).await?);
        }
        if results.is_empty() {
            for query in queries
                .fallback
                .iter()
                .filter(|query| query.indexer() == indexer)
            {
                let found = self.run_query(query).await?;
                if !found.is_empty() {
                    results.extend(found);
                    break;
                }
            }
        }
        Ok(results)
    }

    async fn run_query(&self, query: &Query) -> Result<Vec<Torrent>, Error> {
        if let Some(cached) = self.cached(query) {
            return Ok(cached);
        }
        let bytes = self.fetch_with_retry(query).await?;
        let results = parse_response(query, &bytes)?;
        lock_cache(&self.cache).insert(
            query.clone(),
            CacheEntry {
                at: self.clock.now(),
                results: results.clone(),
            },
        );
        Ok(results)
    }

    async fn fetch_with_retry(&self, query: &Query) -> Result<Vec<u8>, Error> {
        match self.transport.fetch(query).await {
            Ok(bytes) => Ok(bytes),
            Err(error) if is_retryable(&error) => {
                let delay = match &error {
                    Error::RateLimited { retry_after, .. } => *retry_after,
                    _ => DEFAULT_BACKOFF,
                };
                tokio::time::sleep(delay).await;
                self.transport.fetch(query).await
            }
            Err(error) => Err(error),
        }
    }

    fn cached(&self, query: &Query) -> Option<Vec<Torrent>> {
        let cache = lock_cache(&self.cache);
        let entry = cache.get(query)?;
        let age = self.clock.now().saturating_duration_since(entry.at);
        (age < CACHE_TTL).then(|| entry.results.clone())
    }
}

fn is_retryable(error: &Error) -> bool {
    matches!(
        error,
        Error::RateLimited { .. } | Error::Http { status: 503, .. } | Error::Transport { .. }
    )
}

/// Build the single request URL for a query from the configured base URL.
///
/// # Errors
///
/// Returns [`Error::BaseUrl`] if the configured base URL is not an http(s)
/// URL with a host and no credentials, path, query or fragment.
pub fn query_url(query: &Query, config: &Config) -> Result<String, Error> {
    match query {
        Query::Nyaa { keyword } => {
            let mut url = base_url(&config.nyaa_base_url)?;
            url.query_pairs_mut()
                .append_pair("page", "rss")
                .append_pair("f", "0")
                .append_pair("c", "1_2")
                .append_pair("q", keyword)
                .append_pair("u", "")
                .append_pair("m", "");
            Ok(url.to_string())
        }
        Query::AnimeTosho {
            keyword,
            aid,
            eid,
            season,
            episode,
        } => {
            let mut url = base_url(&config.animetosho_base_url)?
                .join("/json")
                .map_err(|error| Error::BaseUrl(error.to_string()))?;
            {
                let mut pairs = url.query_pairs_mut();
                if let Some(keyword) = keyword {
                    pairs.append_pair("q", keyword);
                }
                if let Some(aid) = aid {
                    pairs.append_pair("aid", &aid.to_string());
                }
                if let Some(eid) = eid {
                    pairs.append_pair("eid", &eid.to_string());
                }
                if let Some(season) = season {
                    pairs.append_pair("season", &season.to_string());
                }
                if let Some(episode) = episode {
                    pairs.append_pair("ep", &episode.to_string());
                }
                pairs.append_pair("limit", "75");
            }
            Ok(url.to_string())
        }
    }
}

fn base_url(raw: &str) -> Result<Url, Error> {
    let raw = raw.trim();
    let candidate = if raw.contains("://") {
        raw.to_owned()
    } else {
        format!("https://{raw}")
    };
    let url = Url::parse(&candidate).map_err(|error| Error::BaseUrl(error.to_string()))?;
    let valid = matches!(url.scheme(), "http" | "https")
        && url.host_str().is_some()
        && url.username().is_empty()
        && url.password().is_none()
        && url.path() == "/"
        && url.query().is_none()
        && url.fragment().is_none();
    if !valid {
        return Err(Error::BaseUrl(raw.to_owned()));
    }
    Ok(url)
}

fn parse_response(query: &Query, bytes: &[u8]) -> Result<Vec<Torrent>, Error> {
    let body = String::from_utf8_lossy(bytes);
    match query {
        Query::Nyaa { .. } => nyaa::parse_rss(&body),
        Query::AnimeTosho { .. } => animetosho::parse_search(&body),
    }
}

/// The production transport: one `net` client, shared with the rest of the app.
#[derive(Debug, Clone)]
pub struct NetTransport {
    client: net::Client,
    config: Config,
}

impl NetTransport {
    /// Wrap a shared HTTP client and the configured indexer base URLs.
    #[must_use]
    pub const fn new(client: net::Client, config: Config) -> Self {
        Self { client, config }
    }
}

impl Transport for NetTransport {
    async fn fetch(&self, query: &Query) -> Result<Vec<u8>, Error> {
        let url = query_url(query, &self.config)?;
        let request = net::Request::new(net::Method::Get, url);
        let response = self
            .client
            .read(request)
            .await
            .map_err(|error| net_error(query.indexer(), error))?;
        response.bytes().await.map_err(|error| Error::Transport {
            indexer: query.indexer(),
            message: error.to_string(),
        })
    }
}

fn net_error(indexer: Indexer, error: net::Error) -> Error {
    match error {
        net::Error::RateLimited { retry_after, .. } => Error::RateLimited {
            indexer,
            retry_after,
        },
        net::Error::Http { status, .. } => Error::Http { indexer, status },
        other => Error::Transport {
            indexer,
            message: other.to_string(),
        },
    }
}

#[expect(
    clippy::expect_used,
    reason = "the query cache mutex is only held for short, infallible map operations"
)]
fn lock_cache(
    cache: &Mutex<HashMap<Query, CacheEntry>>,
) -> MutexGuard<'_, HashMap<Query, CacheEntry>> {
    cache.lock().expect("cache mutex is not poisoned")
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<NetTransport>();
    assert_send::<Searcher<NetTransport>>();
};
