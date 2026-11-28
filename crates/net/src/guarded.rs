//! The guarded client for extension traffic (ADR 0015).
//!
//! Each extension gets its own `reqwest` client, because the validating
//! resolver and the redirect policy both need that extension's declared
//! hosts and LAN host exceptions. App traffic keeps using [`Client`](crate::Client).

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use tracing::warn;

use crate::{
    Error, ExtensionHosts, HttpError, Request, Response,
    activity::{ActiveRequests, InFlight},
    client::{CONNECT_TIMEOUT, REQUEST_TIMEOUT, install_tls_provider, validate_proxy},
    guard::{BlockReason, Blocked, Loopback, MAX_REDIRECTS, Policy},
    hosts::NormalizedHost,
};

/// Where the validating resolver gets its answers before checking them.
#[derive(Debug, Clone)]
enum Dns {
    System,
    #[cfg(feature = "test-util")]
    Static(Arc<HashMap<String, Vec<IpAddr>>>),
}

impl Dns {
    async fn lookup(&self, name: &str) -> std::io::Result<Vec<IpAddr>> {
        match self {
            Self::System => Ok(tokio::net::lookup_host((name, 0))
                .await?
                .map(|addr| addr.ip())
                .collect()),
            #[cfg(feature = "test-util")]
            Self::Static(table) => table
                .get(&name.to_ascii_lowercase())
                .cloned()
                .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, name.to_owned())),
        }
    }
}

/// Resolves through [`Dns`] and refuses any answer containing a blocked
/// address. hyper connects only to the addresses returned here, which
/// closes DNS rebinding.
#[derive(Debug)]
struct ValidatingResolver {
    policy: Arc<Policy>,
    dns: Dns,
}

impl Resolve for ValidatingResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let policy = Arc::clone(&self.policy);
        let dns = self.dns.clone();
        Box::pin(async move {
            let addrs = dns.lookup(name.as_str()).await?;
            policy.check_resolved(name.as_str(), &addrs)?;
            let addrs: Addrs = Box::new(addrs.into_iter().map(|ip| SocketAddr::new(ip, 0)));
            Ok(addrs)
        })
    }
}

/// Configures a [`GuardedClient`].
#[derive(Debug, Clone)]
pub struct GuardedClientBuilder {
    proxy_url: Option<String>,
    loopback: Loopback,
    dns: Dns,
}

impl Default for GuardedClientBuilder {
    fn default() -> Self {
        Self {
            proxy_url: None,
            loopback: Loopback::Blocked,
            dns: Dns::System,
        }
    }
}

impl GuardedClientBuilder {
    /// Route extension traffic through the global outbound proxy (ADR 0010).
    ///
    /// Under `socks5://` the full guard holds. Under `http://` the proxy
    /// resolves targets, so only scheme, names, literals, declared hosts and
    /// redirects are enforced here.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the proxy URL is invalid.
    pub fn proxy(mut self, proxy_url: impl Into<String>) -> Result<Self, Error> {
        self.proxy_url = Some(validate_proxy(proxy_url.into())?);
        Ok(self)
    }

    /// Let loopback addresses through the IP check, so fixtures can serve
    /// from `127.0.0.1`. Names such as `localhost` stay blocked.
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn allow_loopback(mut self) -> Self {
        self.loopback = Loopback::Allowed;
        self
    }

    /// Answer DNS from a fixed table instead of the system resolver. The
    /// guard still checks every answer; unknown names fail to resolve.
    #[cfg(feature = "test-util")]
    #[must_use]
    pub fn static_dns<'a>(
        mut self,
        table: impl IntoIterator<Item = (&'a str, Vec<IpAddr>)>,
    ) -> Self {
        let table = table
            .into_iter()
            .map(|(name, addrs)| (name.to_ascii_lowercase(), addrs))
            .collect();
        self.dns = Dns::Static(Arc::new(table));
        self
    }

    /// Build the client. Per-extension transports are created on first use.
    ///
    /// # Errors
    ///
    /// Infallible today; kept fallible to match [`ClientBuilder::build`](crate::ClientBuilder::build).
    pub fn build(self) -> Result<GuardedClient, Error> {
        install_tls_provider();
        Ok(GuardedClient {
            inner: Arc::new(GuardedInner {
                state: Mutex::new(State {
                    proxy_url: self.proxy_url,
                    clients: HashMap::new(),
                }),
                loopback: self.loopback,
                dns: self.dns,
                activity: tokio::sync::watch::Sender::new(ActiveRequests::new()),
            }),
        })
    }
}

#[derive(Debug)]
struct Transport {
    policy: Arc<Policy>,
    http: reqwest::Client,
}

#[derive(Debug)]
struct State {
    proxy_url: Option<String>,
    /// One transport per extension id, rebuilt when its hosts or the proxy change.
    clients: HashMap<String, Arc<Transport>>,
}

#[derive(Debug)]
struct GuardedInner {
    state: Mutex<State>,
    loopback: Loopback,
    dns: Dns,
    activity: tokio::sync::watch::Sender<ActiveRequests>,
}

/// The HTTP client for extension traffic: every request is held to the
/// outbound guard and the extension's declared hosts. Cloning shares it.
#[derive(Debug, Clone)]
pub struct GuardedClient {
    inner: Arc<GuardedInner>,
}

impl GuardedClient {
    /// A guarded client with no proxy.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the HTTP stack cannot be initialized.
    pub fn new() -> Result<Self, Error> {
        Self::builder().build()
    }

    /// Start configuring a guarded client.
    #[must_use]
    pub fn builder() -> GuardedClientBuilder {
        GuardedClientBuilder::default()
    }

    /// Set or clear the global outbound proxy. Extension transports are
    /// rebuilt on their next request; in-flight requests finish on the old one.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Build`] if the proxy URL is invalid.
    pub fn set_proxy(&self, proxy_url: Option<&str>) -> Result<(), Error> {
        let proxy_url = proxy_url
            .map(|url| validate_proxy(url.to_owned()))
            .transpose()?;
        let mut state = lock(&self.inner.state);
        state.proxy_url = proxy_url;
        state.clients.clear();
        Ok(())
    }

    /// Send one request for an extension. Any HTTP status is returned as a
    /// [`Response`]; nothing is retried.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Blocked`] if the URL, a resolved address or a
    /// redirect breaks the outbound guard, [`Error::InvalidRequest`] for a
    /// malformed request, and [`Error::Transport`] if no response arrived.
    pub async fn send(&self, hosts: &ExtensionHosts, request: Request) -> Result<Response, Error> {
        let in_flight = InFlight::begin(&self.inner.activity);
        let request = request.into_reqwest()?;
        let transport = self.transport(hosts)?;
        if let Err(blocked) = transport.policy.check_url(request.url()) {
            return Err(refuse(hosts, blocked));
        }
        let host = request.url().host_str().unwrap_or_default().to_owned();
        match transport.http.execute(request).await {
            Ok(response) => Ok(Response::new(response, in_flight)),
            Err(source) => Err(match find_blocked(&source) {
                Some(blocked) => refuse(hosts, blocked),
                None => Error::Transport {
                    host,
                    source: HttpError::new(source),
                },
            }),
        }
    }

    fn transport(&self, hosts: &ExtensionHosts) -> Result<Arc<Transport>, Error> {
        let mut state = lock(&self.inner.state);
        if let Some(transport) = state.clients.get(hosts.extension_id())
            && transport.policy.hosts == *hosts
        {
            return Ok(Arc::clone(transport));
        }
        let transport = Arc::new(self.build_transport(hosts.clone(), state.proxy_url.as_deref())?);
        state
            .clients
            .insert(hosts.extension_id().to_owned(), Arc::clone(&transport));
        Ok(transport)
    }

    fn build_transport(
        &self,
        hosts: ExtensionHosts,
        proxy_url: Option<&str>,
    ) -> Result<Transport, Error> {
        let build_error = |source: reqwest::Error| Error::Build {
            source: HttpError::new(source),
        };
        let proxy_host = proxy_url
            .and_then(|url| reqwest::Url::parse(url).ok())
            .and_then(|url| NormalizedHost::from_url(&url));
        let policy = Arc::new(Policy {
            hosts,
            proxy_host,
            loopback: self.inner.loopback,
        });
        let redirect_policy = Arc::clone(&policy);
        let mut builder = reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .dns_resolver(ValidatingResolver {
                policy: Arc::clone(&policy),
                dns: self.inner.dns.clone(),
            })
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                // `previous` starts with the original URL, so 11 entries means an 11th hop.
                if attempt.previous().len() > MAX_REDIRECTS {
                    let host = NormalizedHost::from_url(attempt.url())
                        .map_or_default(|host| host.to_string());
                    return attempt.error(Blocked {
                        host,
                        reason: BlockReason::TooManyRedirects,
                    });
                }
                match redirect_policy.check_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(blocked) => attempt.error(blocked),
                }
            }));
        builder = match proxy_url {
            Some(url) => builder.proxy(reqwest::Proxy::all(url).map_err(build_error)?),
            // Without this, HTTP_PROXY/HTTPS_PROXY/ALL_PROXY would route around the resolver.
            None => builder.no_proxy(),
        };
        let http = builder.build().map_err(build_error)?;
        Ok(Transport { policy, http })
    }
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<GuardedClient>();
};

/// Recover a refusal raised inside reqwest's resolver or redirect policy.
fn find_blocked(error: &reqwest::Error) -> Option<Blocked> {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(error);
    while let Some(error) = source {
        if let Some(blocked) = error.downcast_ref::<Blocked>() {
            return Some(blocked.clone());
        }
        source = error.source();
    }
    None
}

fn refuse(hosts: &ExtensionHosts, blocked: Blocked) -> Error {
    warn!(
        extension = hosts.extension_id(),
        host = %blocked.host,
        reason = %blocked.reason,
        "outbound guard refused an extension request"
    );
    Error::Blocked {
        host: blocked.host,
        reason: blocked.reason,
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(PoisonError::into_inner)
}
