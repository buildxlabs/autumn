//! The identity of an installed extension, shared by `extensions`, `search`
//! and the UI routes.

use std::fmt;

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
