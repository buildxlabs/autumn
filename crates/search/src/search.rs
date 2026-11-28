//! Search orchestration: URL construction, budget, retry, cache and dedupe.
//!
//! Each indexer, built-in or extension, runs in its own lane and the lanes
//! run concurrently. Numbered queries come first; keyword fallbacks only run
//! when that indexer found nothing. Results are cached for 60 seconds and
//! deduped by infohash in lane order (ADR 0011).

use std::{
    collections::{HashMap, HashSet},
    future::Future,
    pin::Pin,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

use domain::ExtensionId;
use extensions::ProviderRequest;
use url::Url;

use crate::{
    Error, Indexer, LaneFailure, Query, QueryPlan, SearchOutcome, SearchRequest, Torrent,
    animetosho, nyaa, plan,
    provider::{
        NoProviders, ProviderMeta, ProviderSet, keyword_request, lane_metas, structured_request,
    },
    validation::validate_rows,
};

/// `Nyaa`'s default base URL; a mirror can be supplied in [`Config`].
pub const NYAA_BASE_URL: &str = "https://nyaa.si";
/// The live `AnimeTosho` successor feed, not the retired `.org` host.
pub const ANIMETOSHO_BASE_URL: &str = "https://feed.animetosho.net";
/// How long a query's results stay usable. Extension rows are cached under
/// `(id, version, request)`, so an update cannot serve stale results, and a
/// failure is never cached.
pub const CACHE_TTL: Duration = Duration::from_secs(60);
/// How long a provider call may take before its lane is dropped (#264 Q2).
///
/// Dropping the call cancels it; it is not a VM kill and does not count toward
/// extension auto-disable. The other lanes still return their results.
pub const PROVIDER_DEADLINE: Duration = Duration::from_secs(10);
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

/// Which indexer lanes a search runs (#264 Q3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum LaneFilter {
    /// Every available lane, including extensions once they join search.
    #[default]
    All,
    /// Only the built-in indexers, for unattended availability badges.
    BuiltIn,
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

/// A provider call's cache key: id, version and the exact request (#264 Q5).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct ProviderCacheKey {
    id: ExtensionId,
    version: String,
    fingerprint: String,
}

impl ProviderCacheKey {
    fn new(meta: &ProviderMeta, request: &ProviderRequest) -> Self {
        Self {
            id: meta.id.clone(),
            version: meta.version.clone(),
            // Debug is stable within a build and only a cache identity: a
            // format change costs a refetch, never a wrong answer.
            fingerprint: format!("{request:?}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum CacheKey {
    Query(Query),
    Extension(ProviderCacheKey),
}

type LaneFuture<'a> = Pin<Box<dyn Future<Output = Result<Vec<Torrent>, Error>> + Send + 'a>>;

/// Runs queries against both built-in indexers and the extension lanes within
/// the #181 budget.
#[derive(Debug)]
pub struct Searcher<T: Transport, C: Clock = SystemClock> {
    transport: T,
    cache: Mutex<HashMap<CacheKey, CacheEntry>>,
    clock: C,
    providers: Arc<dyn ProviderSet>,
    provider_deadline: Duration,
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
            providers: Arc::new(NoProviders),
            provider_deadline: PROVIDER_DEADLINE,
        }
    }

    /// Add the provider set used for `LaneFilter::All` searches.
    ///
    /// The lane set is read when each search starts. Every provider gets one
    /// `search(request)` call, validated and deduped after the built-ins.
    /// Availability badges pass `LaneFilter::BuiltIn` and never call a provider.
    #[must_use]
    pub fn with_providers(mut self, providers: Arc<dyn ProviderSet>) -> Self {
        self.providers = providers;
        self
    }

    /// Override the provider deadline, for tests that must not wait 10 s.
    #[must_use]
    pub fn with_provider_deadline(mut self, deadline: Duration) -> Self {
        self.provider_deadline = deadline;
        self
    }

    /// Drop every cached result for `id`, on extension update, disable or
    /// removal (#264 Q8). Other extensions' cached rows are left alone.
    pub fn purge_provider(&self, id: &ExtensionId) {
        lock_cache(&self.cache)
            .retain(|key, _| !matches!(key, CacheKey::Extension(provider) if &provider.id == id));
    }

    /// Search both indexers concurrently and return deduped results.
    ///
    /// A failed lane is reported in [`SearchOutcome::failures`] while the
    /// other lanes' results still return (ADR 0011).
    ///
    /// # Errors
    ///
    /// Returns [`Error::AllLanesFailed`] when every lane failed, or when the
    /// filter selected no lanes.
    pub async fn search(&self, request: &SearchRequest) -> Result<SearchOutcome, Error> {
        self.search_with_filter(request, LaneFilter::All).await
    }

    /// Like [`Self::search`], restricted to the lanes in `filter`.
    ///
    /// # Errors
    ///
    /// As [`Self::search`].
    pub async fn search_with_filter(
        &self,
        request: &SearchRequest,
        filter: LaneFilter,
    ) -> Result<SearchOutcome, Error> {
        let queries = plan(request);
        let mut lanes: Vec<(Indexer, LaneFuture<'_>)> = vec![
            (
                Indexer::Nyaa,
                Box::pin(self.run_indexer(&queries, Indexer::Nyaa)),
            ),
            (
                Indexer::AnimeTosho,
                Box::pin(self.run_indexer(&queries, Indexer::AnimeTosho)),
            ),
        ];
        // Badges stay on the built-ins; only `All` calls user-installed hosts.
        if filter == LaneFilter::All {
            self.add_provider_lanes(&mut lanes, &structured_request(request), false);
        }
        tracing::debug!(?filter, lanes = lanes.len(), "running indexer lanes");
        self.run_lanes(lanes).await
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    fn add_provider_lanes<'a>(
        &'a self,
        lanes: &mut Vec<(Indexer, LaneFuture<'a>)>,
        request: &ProviderRequest,
        refresh: bool,
    ) {
        for meta in lane_metas(self.providers.as_ref()) {
            let indexer = Indexer::Extension {
                id: meta.id.clone(),
                name: Arc::clone(&meta.name),
            };
            lanes.push((
                indexer,
                Box::pin(self.run_extension(meta, request.clone(), refresh)),
            ));
        }
    }

    async fn run_lanes(
        &self,
        lanes: Vec<(Indexer, LaneFuture<'_>)>,
    ) -> Result<SearchOutcome, Error> {
        let indexers: Vec<Indexer> = lanes.iter().map(|(indexer, _)| indexer.clone()).collect();
        let results = futures::future::join_all(lanes.into_iter().map(|(_, future)| future)).await;
        outcome(indexers.into_iter().zip(results))
    }

    /// One provider call per search, under the deadline, with no host retry.
    ///
    /// # Errors
    ///
    /// Returns the provider or validation error for this lane only; `outcome`
    /// decides whether the whole search fails.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    async fn run_extension(
        &self,
        meta: ProviderMeta,
        request: ProviderRequest,
        refresh: bool,
    ) -> Result<Vec<Torrent>, Error> {
        let key = ProviderCacheKey::new(&meta, &request);
        if !refresh && let Some(cached) = self.cached(&CacheKey::Extension(key.clone())) {
            return Ok(cached);
        }
        let call = self.providers.search(meta.id.clone(), request);
        let rows = tokio::time::timeout(self.provider_deadline, call)
            .await
            .map_err(|_| Error::ProviderTimeout {
                id: meta.id.clone(),
            })?
            .map_err(|error| Error::Provider {
                id: meta.id.clone(),
                message: error.to_string(),
            })?;
        let results = validate_rows(rows, &meta)?;
        lock_cache(&self.cache).insert(
            CacheKey::Extension(key),
            CacheEntry {
                at: self.clock.now(),
                results: results.clone(),
            },
        );
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

    /// One keyword-only search on both indexers (no numbered or id-keyed
    /// queries, no keyword fallback), within the same budget and 60 s cache.
    /// `refresh` bypasses the cache for this call. Deduped by infohash.
    ///
    /// # Errors
    ///
    /// As [`Self::search`].
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    pub async fn search_keyword(
        &self,
        keyword: &str,
        refresh: bool,
    ) -> Result<SearchOutcome, Error> {
        self.search_keyword_with_filter(keyword, refresh, LaneFilter::All)
            .await
    }

    /// Like [`Self::search_keyword`], restricted to the lanes in `filter`.
    ///
    /// # Errors
    ///
    /// As [`Self::search`].
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    pub async fn search_keyword_with_filter(
        &self,
        keyword: &str,
        refresh: bool,
        filter: LaneFilter,
    ) -> Result<SearchOutcome, Error> {
        let keyword = keyword.trim();
        let nyaa = Query::Nyaa {
            keyword: keyword.to_owned(),
        };
        let tosho = Query::anime_tosho_fallback(keyword);
        let mut lanes: Vec<(Indexer, LaneFuture<'_>)> = vec![
            (
                Indexer::Nyaa,
                Box::pin(self.run_query_cached(&nyaa, refresh)),
            ),
            (
                Indexer::AnimeTosho,
                Box::pin(self.run_query_cached(&tosho, refresh)),
            ),
        ];
        if filter == LaneFilter::All {
            self.add_provider_lanes(&mut lanes, &keyword_request(keyword), refresh);
        }
        tracing::debug!(
            ?filter,
            lanes = lanes.len(),
            "running keyword indexer lanes"
        );
        self.run_lanes(lanes).await
    }

    async fn run_query(&self, query: &Query) -> Result<Vec<Torrent>, Error> {
        self.run_query_cached(query, false).await
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    async fn run_query_cached(&self, query: &Query, refresh: bool) -> Result<Vec<Torrent>, Error> {
        if !refresh && let Some(cached) = self.cached(&CacheKey::Query(query.clone())) {
            return Ok(cached);
        }
        let bytes = self.fetch_with_retry(query).await?;
        let results = parse_response(query, &bytes)?;
        lock_cache(&self.cache).insert(
            CacheKey::Query(query.clone()),
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

    fn cached(&self, key: &CacheKey) -> Option<Vec<Torrent>> {
        let cache = lock_cache(&self.cache);
        let entry = cache.get(key)?;
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

/// Merge concurrent lanes in a fixed order, keeping the first duplicate and
/// reporting failed lanes instead of failing the whole search (ADR 0011).
fn outcome(
    lanes: impl IntoIterator<Item = (Indexer, Result<Vec<Torrent>, Error>)>,
) -> Result<SearchOutcome, Error> {
    let mut merged = SearchOutcome::default();
    let mut lane_count = 0;
    for (indexer, yielded) in lanes {
        lane_count += 1;
        match yielded {
            Ok(torrents) => merged.torrents.extend(torrents),
            Err(error) => {
                tracing::warn!(indexer = %indexer.label(), %error, "indexer lane failed");
                merged.failures.push(LaneFailure { indexer, error });
            }
        }
    }
    if lane_count == 0 || merged.failures.len() == lane_count {
        return Err(Error::AllLanesFailed);
    }
    let mut seen = HashSet::new();
    merged
        .torrents
        .retain(|torrent| seen.insert(torrent.infohash.to_ascii_lowercase()));
    Ok(merged)
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

/// Locks the query cache. A poisoned lock still holds a usable map, since
/// every write is a single insert or remove.
fn lock_cache(
    cache: &Mutex<HashMap<CacheKey, CacheEntry>>,
) -> MutexGuard<'_, HashMap<CacheKey, CacheEntry>> {
    cache.lock().unwrap_or_else(PoisonError::into_inner)
}

const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<NetTransport>();
    assert_send::<Searcher<NetTransport>>();
};
