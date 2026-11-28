//! The identity of an installed extension, shared by `extensions`, `search`
//! and the UI routes.

use std::{fmt, time::SystemTime};

use serde::{Deserialize, Serialize};

/// An extension's manifest slug, unique across every installed extension.
///
/// It names the extension in logs, Luau stack frames (`my-ext:42`), search
/// labels and routes. The slug rules are checked when a package is
/// installed, not here.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ExtensionId(String);

impl ExtensionId {
    /// Wraps a manifest slug.
    #[must_use]
    pub fn new(slug: impl Into<String>) -> Self {
        Self(slug.into())
    }

    /// The slug.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for ExtensionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// The kind of failure recorded in an extension's health (#269 §3).
///
/// A call the user or search cancelled (including the 10 s search deadline)
/// is never recorded. Only [`Self::CpuLimit`] and [`Self::MemoryLimit`]
/// count toward the three strikes that auto-disable an extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum FailureKind {
    /// The call used more than 1 s of Luau CPU time. Counts as a strike.
    CpuLimit,
    /// The call ran longer than 30 s, host waits included.
    WallTimeout,
    /// The VM went over its 32 MiB memory cap. Counts as a strike.
    MemoryLimit,
    /// The call made more than 8 HTTP requests.
    RequestLimit,
    /// The script raised an error or failed to compile, `search` returned
    /// something other than an array, or the chunk returned no provider.
    RuntimeError,
    /// The outbound guard refused a request: an undeclared host, a private
    /// address, or a non-HTTPS URL.
    NetworkBlocked,
}

impl FailureKind {
    /// Whether this kind counts toward the three strikes.
    #[must_use]
    pub const fn strikes(self) -> bool {
        matches!(self, Self::CpuLimit | Self::MemoryLimit)
    }
}

/// An extension's last recorded failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExtensionFailure {
    /// What went wrong.
    pub kind: FailureKind,
    /// The error message shown to the user, at most 1 KiB.
    pub message: String,
    /// When the failure was recorded.
    pub at: SystemTime,
}

impl ExtensionFailure {
    /// A failure of `kind` with `message`, recorded `at`.
    #[must_use]
    pub fn new(kind: FailureKind, message: impl Into<String>, at: SystemTime) -> Self {
        Self {
            kind,
            message: message.into(),
            at,
        }
    }
}

/// An installed extension's health, as the Extensions screen badges it.
///
/// Health is independent of the user's enabled toggle. `Failing` clears on
/// the next successful call, on Clear error, on Re-enable, and on removal.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ExtensionHealth {
    /// No failure since the last success or clear.
    #[default]
    Healthy,
    /// The last call failed; the extension still runs.
    Failing(ExtensionFailure),
    /// Three CPU or memory kills in a row turned the extension off until the
    /// user re-enables it. Carries the last failure, unless it was cleared.
    AutoDisabled(Option<ExtensionFailure>),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn displays_and_serializes_as_the_bare_slug() {
        let id = ExtensionId::new("my-ext");
        assert_eq!(id.to_string(), "my-ext");
        assert_eq!(id.as_str(), "my-ext");
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, "\"my-ext\"");
        assert_eq!(serde_json::from_str::<ExtensionId>(&json).unwrap(), id);
    }
}
