//! Shared HTTP client for Autumn: per-host rate limiting and retries.
//!
//! Every outbound request goes through one [`Client`], which applies a
//! per-host token bucket and retries reads with backoff. Mutations are sent
//! once. See ARCHITECTURE §2 and §6. This is a back-end crate: no GPUI.

mod cache;
mod client;
mod error;
mod limiter;
mod request;

pub use cache::PosterCache;
pub use client::{ANILIST_HOST, Client, ClientBuilder, HostKind, Lockout};
pub use error::{Error, HttpError};
pub use request::{Method, Request, Response};
