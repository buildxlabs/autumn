//! Declared hosts and LAN host exceptions an extension request carries (ADR 0015).

use std::{fmt, net::IpAddr, str::FromStr};

use reqwest::Url;
use url::Host;

use crate::Error;

/// A host after lowercasing and stripping one trailing dot.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum NormalizedHost {
    Domain(String),
    Ip(IpAddr),
}

impl NormalizedHost {
    pub(crate) fn from_url(url: &Url) -> Option<Self> {
        Some(match url.host()? {
            Host::Domain(domain) => Self::domain(domain),
            Host::Ipv4(ip) => Self::Ip(IpAddr::V4(ip)),
            Host::Ipv6(ip) => Self::Ip(IpAddr::V6(ip)),
        })
    }

    pub(crate) fn domain(name: &str) -> Self {
        Self::Domain(name.strip_suffix('.').unwrap_or(name).to_ascii_lowercase())
    }

    /// `localhost`, `*.localhost` (RFC 6761) and `*.local` (RFC 6762).
    pub(crate) fn is_local_name(&self) -> bool {
        match self {
            Self::Domain(name) => {
                let tld = name.rsplit('.').next().unwrap_or(name);
                name == "localhost"
                    || name.ends_with(".localhost")
                    || (tld == "local" && name != "local")
            }
            Self::Ip(_) => false,
        }
    }

    /// Parse a bare host (a domain or an IP literal) with `url`'s rules.
    fn parse(input: &str) -> Option<Self> {
        if let Ok(ip) = input.parse::<IpAddr>() {
            return Some(Self::Ip(ip));
        }
        if input.is_empty() || input.contains(|c: char| ":/?#@[]\\*%".contains(c)) {
            return None;
        }
        let url = Url::parse(&format!("http://{input}/")).ok()?;
        let host = Self::from_url(&url)?;
        let Self::Domain(name) = &host else {
            // `url` turned an integer or octal spelling into an IPv4 address.
            return Some(host);
        };
        if name.split('.').any(str::is_empty) {
            return None;
        }
        Some(host)
    }
}

impl fmt::Display for NormalizedHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Domain(name) => f.write_str(name),
            Self::Ip(ip) => write!(f, "{ip}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum Pattern {
    Exact(NormalizedHost),
    /// `*.example.com`: any subdomain of the suffix, at any depth, but not the suffix itself.
    Subdomains(String),
}

/// A host pattern from an extension's manifest: an exact host or `*.example.com`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DeclaredHost(Pattern);

impl DeclaredHost {
    /// Validate a manifest host pattern.
    ///
    /// Accepts an exact domain, an IP literal, or `*.` followed by a domain
    /// of at least two labels. A bare `*` and a wildcard over a single label
    /// such as `*.com` are rejected.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidHostPattern`] for an empty, malformed or
    /// over-broad pattern.
    pub fn parse(pattern: &str) -> Result<Self, Error> {
        let invalid = |reason: &'static str| Error::InvalidHostPattern {
            pattern: pattern.to_owned(),
            reason,
        };
        if pattern == "*" {
            return Err(invalid("a bare * matches every host"));
        }
        if let Some(suffix) = pattern.strip_prefix("*.") {
            let Some(NormalizedHost::Domain(suffix)) = NormalizedHost::parse(suffix) else {
                return Err(invalid("a wildcard must be followed by a domain"));
            };
            if !suffix.contains('.') {
                return Err(invalid("a wildcard must sit under a registrable domain"));
            }
            return Ok(Self(Pattern::Subdomains(suffix)));
        }
        NormalizedHost::parse(pattern)
            .map(|host| Self(Pattern::Exact(host)))
            .ok_or_else(|| invalid("not a valid host name"))
    }

    pub(crate) fn matches(&self, host: &NormalizedHost) -> bool {
        match (&self.0, host) {
            (Pattern::Exact(exact), host) => exact == host,
            (Pattern::Subdomains(suffix), NormalizedHost::Domain(name)) => name
                .strip_suffix(suffix.as_str())
                .and_then(|label| label.strip_suffix('.'))
                .is_some_and(|label| !label.is_empty()),
            (Pattern::Subdomains(_), NormalizedHost::Ip(_)) => false,
        }
    }
}

impl FromStr for DeclaredHost {
    type Err = Error;

    fn from_str(pattern: &str) -> Result<Self, Self::Err> {
        Self::parse(pattern)
    }
}

impl fmt::Display for DeclaredHost {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.0 {
            Pattern::Exact(host) => write!(f, "{host}"),
            Pattern::Subdomains(suffix) => write!(f, "*.{suffix}"),
        }
    }
}

/// A user-approved host, optionally limited to one port, that may reach
/// private, loopback and `.local` addresses for one extension.
///
/// Given by name, it still cannot reach link-local or multicast addresses;
/// given as an IP literal, that exact address is allowed.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LanHostException {
    host: NormalizedHost,
    port: Option<u16>,
}

impl LanHostException {
    /// Parse `host`, `host:port`, an IP literal or `[ipv6]:port`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidHostPattern`] if the host or port is malformed.
    pub fn parse(input: &str) -> Result<Self, Error> {
        let invalid = || Error::InvalidHostPattern {
            pattern: input.to_owned(),
            reason: "not a host or host:port",
        };
        let (host, port) = if let Some(rest) = input.strip_prefix('[') {
            let (host, rest) = rest.split_once(']').ok_or_else(invalid)?;
            match rest.strip_prefix(':') {
                Some(port) => (host, Some(port)),
                None if rest.is_empty() => (host, None),
                None => return Err(invalid()),
            }
        } else if input.matches(':').count() == 1 {
            input
                .split_once(':')
                .map(|(host, port)| (host, Some(port)))
                .ok_or_else(invalid)?
        } else {
            (input, None)
        };
        let port = port
            .map(|port| port.parse::<u16>().map_err(|_| invalid()))
            .transpose()?;
        let host = NormalizedHost::parse(host).ok_or_else(invalid)?;
        Ok(Self { host, port })
    }

    pub(crate) fn host(&self) -> &NormalizedHost {
        &self.host
    }

    pub(crate) fn matches(&self, host: &NormalizedHost, port: Option<u16>) -> bool {
        self.host == *host && self.port.is_none_or(|own| Some(own) == port)
    }
}

impl FromStr for LanHostException {
    type Err = Error;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        Self::parse(input)
    }
}

/// The hosts one extension may contact: its declared hosts plus any LAN
/// host exceptions the user approved for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtensionHosts {
    extension_id: String,
    declared: Vec<DeclaredHost>,
    lan_exceptions: Vec<LanHostException>,
}

impl ExtensionHosts {
    /// Hosts for an extension id, with no LAN host exceptions.
    #[must_use]
    pub fn new(extension_id: impl Into<String>, declared: Vec<DeclaredHost>) -> Self {
        Self {
            extension_id: extension_id.into(),
            declared,
            lan_exceptions: Vec::new(),
        }
    }

    /// Add the user's LAN host exceptions for this extension.
    #[must_use]
    pub fn with_lan_exceptions(mut self, lan_exceptions: Vec<LanHostException>) -> Self {
        self.lan_exceptions = lan_exceptions;
        self
    }

    /// The extension these hosts belong to.
    #[must_use]
    pub fn extension_id(&self) -> &str {
        &self.extension_id
    }

    pub(crate) fn declared(&self) -> &[DeclaredHost] {
        &self.declared
    }

    pub(crate) fn lan_exceptions(&self) -> &[LanHostException] {
        &self.lan_exceptions
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str) -> NormalizedHost {
        NormalizedHost::parse(name).unwrap()
    }

    #[test]
    fn patterns_reject_bare_and_public_suffix_wildcards() {
        for pattern in ["*", "*.com", "*.", "", "*.*.example.com", "a*.example.com"] {
            assert!(
                DeclaredHost::parse(pattern).is_err(),
                "pattern {pattern:?} should be rejected"
            );
        }
        for pattern in [
            "example.com",
            "*.example.com",
            "api.example.com",
            "93.184.216.34",
        ] {
            assert!(
                DeclaredHost::parse(pattern).is_ok(),
                "pattern {pattern:?} should be accepted"
            );
        }
    }

    #[test]
    fn patterns_reject_ports_paths_and_credentials() {
        for pattern in [
            "example.com:443",
            "example.com/path",
            "user@example.com",
            "https://example.com",
            "exa mple.com",
        ] {
            assert!(
                DeclaredHost::parse(pattern).is_err(),
                "pattern {pattern:?} should be rejected"
            );
        }
    }

    #[test]
    fn a_wildcard_matches_subdomains_but_not_the_apex() {
        let pattern = DeclaredHost::parse("*.Example.com.").unwrap();
        assert!(pattern.matches(&host("api.example.com")));
        assert!(pattern.matches(&host("a.b.example.com")));
        assert!(!pattern.matches(&host("example.com")));
        assert!(!pattern.matches(&host("badexample.com")));
        assert!(!pattern.matches(&host("example.com.evil.net")));
    }

    #[test]
    fn exact_patterns_match_after_lowercasing_and_a_trailing_dot() {
        let pattern = DeclaredHost::parse("API.example.com").unwrap();
        assert!(pattern.matches(&NormalizedHost::domain("api.example.com.")));
        assert!(!pattern.matches(&host("www.example.com")));
    }

    #[test]
    fn local_names_are_recognised() {
        for name in [
            "localhost",
            "LOCALHOST.",
            "foo.localhost",
            "nas.local",
            "NAS.LOCAL.",
        ] {
            assert!(
                NormalizedHost::domain(name).is_local_name(),
                "{name} is local"
            );
        }
        for name in ["localhost.example.com", "local.example.com", "example.com"] {
            assert!(
                !NormalizedHost::domain(name).is_local_name(),
                "{name} is not local"
            );
        }
    }

    #[test]
    fn lan_exceptions_parse_hosts_and_ports() {
        let any_port = LanHostException::parse("nas.local").unwrap();
        assert!(any_port.matches(&host("nas.local"), Some(80)));
        let one_port = LanHostException::parse("NAS.local:9117").unwrap();
        assert!(one_port.matches(&host("nas.local"), Some(9117)));
        assert!(!one_port.matches(&host("nas.local"), Some(22)));
        let v6 = LanHostException::parse("[fd00::5]:8080").unwrap();
        assert!(v6.matches(&host("fd00::5"), Some(8080)));
        assert!(LanHostException::parse("fd00::5").is_ok());
        assert!(LanHostException::parse("nas.local:http").is_err());
        assert!(LanHostException::parse("nas.local/path").is_err());
    }
}
