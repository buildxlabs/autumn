//! The outbound guard's rules, shared by the URL check, the resolver and redirects.

use std::{fmt, net::IpAddr};

use reqwest::Url;

use crate::{
    address::{AddressClass, classify},
    hosts::{ExtensionHosts, NormalizedHost},
};

/// Redirect hops a guarded request may follow, matching reqwest's default.
pub(crate) const MAX_REDIRECTS: usize = 10;

/// Which rule of the outbound guard refused a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum BlockReason {
    /// The URL is not `http` or `https`.
    Scheme,
    /// The host is `localhost`, `*.localhost` or `*.local`.
    LocalName,
    /// The host is, or resolves to, an address that is not globally reachable.
    Address(AddressClass),
    /// The host is not one of the extension's declared hosts.
    Undeclared,
    /// The request was redirected more than ten times.
    TooManyRedirects,
}

impl fmt::Display for BlockReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scheme => f.write_str("uses a scheme other than http or https"),
            Self::LocalName => f.write_str("is a local name"),
            Self::Address(class) => write!(f, "is a {class} address"),
            Self::Undeclared => f.write_str("is not a declared host"),
            Self::TooManyRedirects => write!(f, "redirected more than {MAX_REDIRECTS} times"),
        }
    }
}

/// A refusal raised inside reqwest's resolver or redirect policy and
/// recovered from the error's source chain.
#[derive(Debug, Clone, thiserror::Error)]
#[error("blocked: {host} {reason}")]
pub(crate) struct Blocked {
    pub(crate) host: String,
    pub(crate) reason: BlockReason,
}

/// Whether loopback addresses pass the IP check. Only tests allow them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Loopback {
    Blocked,
    Allowed,
}

/// Everything one extension's guarded client checks.
#[derive(Debug)]
pub(crate) struct Policy {
    pub(crate) hosts: ExtensionHosts,
    /// The configured proxy's own host, which the resolver must reach.
    pub(crate) proxy_host: Option<NormalizedHost>,
    pub(crate) loopback: Loopback,
}

impl Policy {
    /// The pre-send and per-redirect check: scheme, names, literals, declared hosts.
    pub(crate) fn check_url(&self, url: &Url) -> Result<(), Blocked> {
        let host = NormalizedHost::from_url(url);
        let label = host
            .as_ref()
            .map_or_else(|| format!("{}:", url.scheme()), ToString::to_string);
        let block = |reason| Blocked {
            host: label.clone(),
            reason,
        };
        if !matches!(url.scheme(), "http" | "https") {
            return Err(block(BlockReason::Scheme));
        }
        let Some(host) = host else {
            return Err(block(BlockReason::Undeclared));
        };
        let port = url.port_or_known_default();
        if self
            .hosts
            .lan_exceptions()
            .iter()
            .any(|exception| exception.matches(&host, port))
        {
            return Ok(());
        }
        if host.is_local_name() {
            return Err(block(BlockReason::LocalName));
        }
        if let NormalizedHost::Ip(ip) = host
            && let Some(class) = self.blocked_class(ip, Exception::None)
        {
            return Err(block(BlockReason::Address(class)));
        }
        if !self
            .hosts
            .declared()
            .iter()
            .any(|declared| declared.matches(&host))
        {
            return Err(block(BlockReason::Undeclared));
        }
        Ok(())
    }

    /// The resolver check: refuse a name if any address it resolved to is blocked.
    pub(crate) fn check_resolved(&self, name: &str, addrs: &[IpAddr]) -> Result<(), Blocked> {
        let host = NormalizedHost::domain(name);
        if self.proxy_host.as_ref() == Some(&host) {
            return Ok(());
        }
        let exception = if self
            .hosts
            .lan_exceptions()
            .iter()
            .any(|exception| *exception.host() == host)
        {
            Exception::ByName
        } else {
            Exception::None
        };
        match addrs
            .iter()
            .find_map(|&ip| self.blocked_class(ip, exception))
        {
            Some(class) => Err(Blocked {
                host: host.to_string(),
                reason: BlockReason::Address(class),
            }),
            None => Ok(()),
        }
    }

    fn blocked_class(&self, ip: IpAddr, exception: Exception) -> Option<AddressClass> {
        let class = classify(ip)?;
        let allowed = (class == AddressClass::Loopback && self.loopback == Loopback::Allowed)
            || (exception == Exception::ByName && class.lan_exception_may_reach());
        (!allowed).then_some(class)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Exception {
    None,
    ByName,
}
