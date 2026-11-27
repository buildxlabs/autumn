//! The shared HTTP client: one `reqwest` client behind per-host limiters.

use std::{
    sync::{Arc, Once, PoisonError, RwLock},
    time::Duration,
};

use httpdate::parse_http_date;
use reqwest::{StatusCode, header::HeaderMap, header::HeaderName, header::RETRY_AFTER};
use tokio::time::{Instant, sleep};
use tracing::{debug, warn};

use crate::{
    Error, HttpError, Request, RequestActivity, Response,
    activity::{ActiveRequests, InFlight},
    limiter::{HostConfig, HostLimiters, RateLimit},
};

/// `AniList`'s GraphQL host, configured with a 25 req/min limiter and a burst of 5.
pub const ANILIST_HOST: &str = "graphql.anilist.co";

const DEFAULT_MAX_RETRIES: u32 = 3;
const DEFAULT_BACKOFF: Duration = Duration::from_millis(250);
const MAX_BACKOFF: Duration = Duration::from_secs(30);
const DEFAULT_LOCKOUT: Duration = Duration::from_secs(60);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
const X_RATE_LIMIT_RESET: HeaderName = HeaderName::from_static("x-ratelimit-reset");
static INSTALL_TLS_PROVIDER: Once = Once::new();

/// How a host's HTTP errors are classified.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostKind {
    /// `AniList`: a 403 means the service is unavailable.
    AniList,
    /// Any other host: use the generic HTTP error variants.
    Generic,
}

/// A host's lockout deadline, so callers can show a rate-limit countdown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Lockout {
    until: Instant,
}

impl Lockout {
    pub(crate) fn new(until: Instant) -> Self {
        Self { until }
    }

    /// How long the host will remain locked out.
    #[must_use]
    pub fn remaining(&self) -> Duration {
        self.until.saturating_duration_since(Instant::now())
    }
}

/// Configures a [`Client`]. Hosts are unlimited unless registered.
#[derive(Debug, Clone)]
pub struct ClientBuilder {
    hosts: Vec<(String, HostConfig)>,
    max_retries: u32,
    base_backoff: Duration,
    connect_timeout: Duration,
    request_timeout: Duration,
    proxy_url: Option<String>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            hosts: Vec::new(),
            max_retries: DEFAULT_MAX_RETRIES,
            base_backoff: DEFAULT_BACKOFF,
            connect_timeout: CONNECT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            proxy_url: None,
        }
    }
}

impl ClientBuilder {
    /// Register `AniList`'s GraphQL host with the shared rate limiter.
    #[must_use]
    pub fn anilist(self) -> Self {
        self.host(ANILIST_HOST, 25, 5, HostKind::AniList)
    }

    /// Register a host with a request rate, a burst size and an error kind.
    #[must_use]
    pub fn host(
        mut self,
        host: impl Into<String>,
        requests_per_minute: u32,
        burst: u32,
        kind: HostKind,
    ) -> Self {
        self.hosts.push((
            host.into().to_ascii_lowercase(),
            HostConfig {
                kind,
                requests_per_minute,
                burst,
            },
        ));
        self
    }

    /// Set how many times a read is retried. Defaults to three.
    #[must_use]
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Set the base delay for exponential backoff. Defaults to 250 ms.
    #[must_use]
    pub fn base_backoff(mut self, base_backoff: Duration) -> Self {
        self.base_backoff = base_backoff;
        self
    }

    /// Set the timeout for establishing a connection. Defaults to 10 s.
    #[must_use]
    pub fn connect_timeout(mut self, connect_timeout: Duration) -> Self {
        self.connect_timeout = connect_timeout;
        self
    }

    /// Set the timeout for a whole request. Defaults to 30 s.
    #[must_use]
    pub fn request_timeout(mut self, request_timeout: Duration) -> Self {
        self.request_timeout = request_timeout;
        self
    }

    /// Route all requests through one outbound proxy (`http://` or
    /// `socks5://`, no embedded credentials); in-flight requests on a
    /// rebuilt client finish on the old one. `None`-by-default.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the proxy URL is invalid or the HTTP
    /// client cannot be initialized.
    pub fn proxy(mut self, proxy_url: impl Into<String>) -> Result<Self, Error> {
        self.proxy_url = Some(validate_proxy(proxy_url.into())?);
        Ok(self)
    }

    /// Build the client with connect and per-request timeouts.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the HTTP client cannot be initialized.
    pub fn build(self) -> Result<Client, Error> {
        // reqwest is built without a default TLS provider so aws-lc's native
        // build is not needed. Install ring once for the whole process.
        INSTALL_TLS_PROVIDER.call_once(|| {
            if let Err(already) = rustls::crypto::ring::default_provider().install_default() {
                debug!(?already, "rustls crypto provider already installed");
            }
        });
        let mut limiters = HostLimiters::default();
        for (host, config) in self.hosts {
            limiters.insert(host, config);
        }
        let connect_timeout = self.connect_timeout;
        let request_timeout = self.request_timeout;
        let proxy = self.proxy_url;
        let http = build_reqwest(connect_timeout, request_timeout, proxy.as_deref())?;
        Ok(Client {
            inner: Arc::new(Inner {
                http: RwLock::new(http),
                limiters,
                activity: tokio::sync::watch::Sender::new(ActiveRequests::new()),
                max_retries: self.max_retries,
                base_backoff: self.base_backoff,
                connect_timeout,
                request_timeout,
            }),
        })
    }
}

#[derive(Debug)]
struct Inner {
    http: RwLock<reqwest::Client>,
    limiters: HostLimiters,
    activity: tokio::sync::watch::Sender<ActiveRequests>,
    max_retries: u32,
    base_backoff: Duration,
    connect_timeout: Duration,
    request_timeout: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SendMode {
    Read,
    Mutation,
}

/// One shared HTTP client. Cloning shares the transport and every limiter.
#[derive(Debug, Clone)]
pub struct Client {
    inner: Arc<Inner>,
}

impl Client {
    /// Create a client with `AniList`'s limiter registered.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the HTTP client cannot be initialized.
    pub fn new() -> Result<Self, Error> {
        Self::builder().anilist().build()
    }

    /// Start with an empty set of registered hosts.
    #[must_use]
    pub fn builder() -> ClientBuilder {
        ClientBuilder::default()
    }

    /// Set or clear the global outbound proxy, rebuilding the underlying
    /// `reqwest` client while keeping host limiters and activity; in-flight
    /// requests finish on the old client (ADR-0010). `proxy_url` is
    /// `http://` or `socks5://`, no embedded credentials.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the proxy URL is invalid or the HTTP
    /// client cannot be rebuilt.
    pub fn set_proxy(&self, proxy_url: Option<&str>) -> Result<(), Error> {
        let proxy = match proxy_url {
            Some(url) => Some(validate_proxy(url.to_owned())?),
            None => None,
        };
        let rebuilt = build_reqwest(
            self.inner.connect_timeout,
            self.inner.request_timeout,
            proxy.as_deref(),
        )?;
        {
            let mut http = write_http(&self.inner.http);
            *http = rebuilt;
        }
        Ok(())
    }

    /// Observe in-flight requests across all clones of this client.
    #[must_use]
    pub fn request_activity(&self) -> RequestActivity {
        RequestActivity::new(self.inner.activity.subscribe())
    }

    /// Observe token waits and server lockouts for a registered host.
    #[must_use]
    pub fn rate_limit(&self, host: &str) -> Option<RateLimit> {
        self.inner
            .limiters
            .get(&host.to_ascii_lowercase())
            .map(|limiter| limiter.subscribe())
    }

    /// The current lockout for a host, if it has been rate limited.
    pub async fn lockout(&self, host: &str) -> Option<Lockout> {
        let host = host.to_ascii_lowercase();
        let limiter = self.inner.limiters.get(&host)?;
        limiter.lockout().await
    }

    /// Send a retryable read request, such as a GraphQL query.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails after the configured retries, or
    /// if the host rate limits it or (for `AniList`) reports an outage.
    pub async fn read(&self, request: Request) -> Result<Response, Error> {
        self.send(request, SendMode::Read).await
    }

    /// Send a mutation exactly once. Mutations are never retried automatically.
    ///
    /// # Errors
    ///
    /// Returns an error if the request fails; the caller can decide whether to
    /// present a retry action to the user.
    pub async fn mutate(&self, request: Request) -> Result<Response, Error> {
        self.send(request, SendMode::Mutation).await
    }

    async fn send(&self, request: Request, mode: SendMode) -> Result<Response, Error> {
        let in_flight = InFlight::begin(&self.inner.activity);
        let retryable = mode == SendMode::Read;
        let host = request.host().to_owned();
        let limiter = self.inner.limiters.get(&host);
        let kind = self.inner.limiters.kind(&host);
        let mut current = request.into_reqwest()?;
        let reusable = current.try_clone();
        let mut retry = 0u32;

        loop {
            if let Some(limiter) = &limiter {
                limiter.acquire().await;
            }

            let http = read_http(&self.inner.http).clone();
            let response = match http.execute(current).await {
                Ok(response) => response,
                Err(source) => {
                    if !retryable || retry >= self.inner.max_retries {
                        return Err(Error::Transport {
                            host,
                            source: HttpError::new(source),
                        });
                    }
                    current = clone_for_retry(reusable.as_ref(), &host)?;
                    retry += 1;
                    let delay = self.backoff(retry);
                    debug!(%host, retry, ?delay, "retrying after transport error");
                    sleep(delay).await;
                    continue;
                }
            };

            let status = response.status();
            if status.is_success() {
                return Ok(Response::new(response, in_flight));
            }

            if status == StatusCode::TOO_MANY_REQUESTS {
                let retry_after = parse_retry_after(response.headers()).unwrap_or(DEFAULT_LOCKOUT);
                if let Some(limiter) = &limiter {
                    limiter.pause(retry_after).await;
                }
                warn!(%host, ?retry_after, "host rate limited the request");
                if retryable && retry < self.inner.max_retries {
                    current = clone_for_retry(reusable.as_ref(), &host)?;
                    retry += 1;
                    // A registered limiter waits out the lockout in acquire();
                    // an unregistered host has no queue, so delay here instead.
                    if limiter.is_none() {
                        sleep(retry_after).await;
                    }
                    continue;
                }
                return Err(Error::RateLimited { host, retry_after });
            }

            if status == StatusCode::FORBIDDEN
                && (kind == Some(HostKind::AniList) || host == ANILIST_HOST)
            {
                return Err(Error::AniListUnavailable);
            }

            if retryable && status.is_server_error() && retry < self.inner.max_retries {
                current = clone_for_retry(reusable.as_ref(), &host)?;
                retry += 1;
                let delay = self.backoff(retry);
                debug!(%host, retry, ?delay, "retrying after server error");
                sleep(delay).await;
                continue;
            }

            return Err(Error::Http {
                host,
                status: status.as_u16(),
            });
        }
    }

    fn backoff(&self, retry: u32) -> Duration {
        let multiplier = 1u32
            .checked_shl(retry.saturating_sub(1))
            .unwrap_or(u32::MAX);
        let delay = self
            .inner
            .base_backoff
            .checked_mul(multiplier)
            .unwrap_or(MAX_BACKOFF)
            .min(MAX_BACKOFF);
        delay.mul_f64(rand::random::<f64>().clamp(0.0, 1.0))
    }
}

fn clone_for_retry(
    template: Option<&reqwest::Request>,
    host: &str,
) -> Result<reqwest::Request, Error> {
    template
        .and_then(reqwest::Request::try_clone)
        .ok_or_else(|| Error::Unretryable {
            host: host.to_owned(),
        })
}

fn parse_retry_after(headers: &HeaderMap) -> Option<Duration> {
    // AniList sends both headers on a 429; honour the later of the two so a
    // long reset is never cut short by a stale Retry-After.
    let header = |name: &HeaderName| headers.get(name).and_then(|value| value.to_str().ok());
    [
        header(&RETRY_AFTER).and_then(parse_retry_after_value),
        header(&X_RATE_LIMIT_RESET).and_then(parse_reset_value),
    ]
    .into_iter()
    .flatten()
    .max()
}

fn parse_retry_after_value(value: &str) -> Option<Duration> {
    let value = value.trim();
    if let Ok(seconds) = value.parse::<u64>() {
        return Some(Duration::from_secs(seconds));
    }
    let date = parse_http_date(value).ok()?;
    date.duration_since(std::time::SystemTime::now()).ok()
}

fn parse_reset_value(value: &str) -> Option<Duration> {
    let reset = value.trim().parse::<u64>().ok()?;
    std::time::SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(reset))?
        .duration_since(std::time::SystemTime::now())
        .ok()
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Client>();
};

/// Build a `reqwest` client with timeouts and an optional outbound proxy.
fn build_reqwest(
    connect_timeout: Duration,
    request_timeout: Duration,
    proxy: Option<&str>,
) -> Result<reqwest::Client, Error> {
    let mut builder = reqwest::Client::builder()
        .connect_timeout(connect_timeout)
        .timeout(request_timeout);
    if let Some(url) = proxy {
        let proxy = reqwest::Proxy::all(url)
            .map_err(HttpError::new)
            .map_err(|source| Error::Build { source })?;
        builder = builder.proxy(proxy);
    }
    builder
        .build()
        .map_err(HttpError::new)
        .map_err(|source| Error::Build { source })
}

/// Reject embedded credentials, require scheme + host, and only allow
/// `http://` or `socks5://` for the global outbound proxy (ADR-0010).
fn validate_proxy(url: String) -> Result<String, Error> {
    let invalid = |message: &str| Error::Build {
        source: HttpError::new(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            message.to_owned(),
        )),
    };
    let parsed = reqwest::Url::parse(&url).map_err(|e| invalid(&e.to_string()))?;
    if !matches!(parsed.scheme(), "http" | "socks5") {
        return Err(invalid("unsupported proxy scheme"));
    }
    if parsed.host_str().is_none_or(str::is_empty) {
        return Err(invalid("proxy URL needs a host"));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(invalid("proxy URLs with embedded credentials are rejected"));
    }
    Ok(url)
}

fn read_http(lock: &RwLock<reqwest::Client>) -> std::sync::RwLockReadGuard<'_, reqwest::Client> {
    lock.read().unwrap_or_else(PoisonError::into_inner)
}

fn write_http(lock: &RwLock<reqwest::Client>) -> std::sync::RwLockWriteGuard<'_, reqwest::Client> {
    lock.write().unwrap_or_else(PoisonError::into_inner)
}
