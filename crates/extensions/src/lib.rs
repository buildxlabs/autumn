//! Extensions: sandboxed Luau torrent providers, one VM per extension behind
//! a single-caller actor (ARCHITECTURE §6, ADR 0003, ADR 0014).
//!
//! [`ExtensionHost`] is the plain async API the engine and `search` use. It
//! knows no GPUI and no `search` types: providers return raw
//! [`ProviderTorrent`] rows and `search` validates them (ADR 0011).

mod actor;
mod error;
mod provider;
mod runtime;

pub use actor::{ExtensionHost, ProviderInfo, ProviderSource};
pub use error::ExtensionError;
pub use provider::{EpisodeQuery, ProviderRequest, ProviderTorrent};
