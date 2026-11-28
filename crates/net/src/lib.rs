//! Shared HTTP client and disk poster cache for Autumn.
//!
//! Every outbound request goes through one [`Client`], which applies a
//! per-host token bucket and retries reads with backoff. Mutations are sent
//! once. [`PosterCache`] stores image downloads under the app cache dir with
//! a size cap shared with metadata blobs. See ARCHITECTURE §2 and §6. This is
//! a back-end crate: no GPUI.
//!
//! Extension traffic goes through [`GuardedClient`] instead, which holds every
//! request to the outbound guard and the extension's declared hosts (ADR 0015).

mod activity;
mod address;
mod cache;
mod client;
mod error;
mod guard;
mod guarded;
mod hosts;
mod limiter;
mod request;

pub use activity::RequestActivity;
pub use address::AddressClass;
pub use cache::PosterCache;
pub use client::{ANILIST_HOST, Client, ClientBuilder, HostKind, Lockout};
pub use error::{Error, HttpError};
pub use guard::BlockReason;
pub use guarded::{GuardedClient, GuardedClientBuilder, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
pub use hosts::{DeclaredHost, ExtensionHosts, LanHostException};
pub use limiter::{RateLimit, RateLimitSnapshot};
pub use request::{Method, Request, Response};
