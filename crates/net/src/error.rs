//! Errors returned by the shared HTTP client and the disk poster cache.

use std::{path::PathBuf, time::Duration};

use thiserror::Error;

use crate::BlockReason;

/// An opaque cause from the HTTP stack.
///
/// Its field is private so the rest of the workspace never names a reqwest
/// type (ARCHITECTURE §3, rule 4).
#[derive(Debug, Error)]
#[error("{message}")]
pub struct HttpError {
    message: String,
    #[source]
    source: Box<dyn std::error::Error + Send + Sync + 'static>,
}

impl HttpError {
    pub(crate) fn new(source: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self {
            message: source.to_string(),
            source: Box::new(source),
        }
    }
}

impl From<HttpError> for Error {
    fn from(source: HttpError) -> Self {
        Self::InvalidRequest { source }
    }
}

/// A failure from an HTTP request or a disk-cache operation.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The outbound guard refused an extension request (ADR 0015).
    #[error("blocked: {host} {reason}")]
    Blocked {
        /// The host, or `scheme:` for a URL without one, that broke the rule.
        host: String,
        /// The rule it broke.
        reason: BlockReason,
    },

    /// An extension request body is larger than the 1 MiB cap.
    #[error("request to {host} is larger than {limit} bytes")]
    RequestTooLarge {
        /// The host the request was addressed to.
        host: String,
        /// The cap in bytes.
        limit: u64,
    },

    /// A response body grew past its cap and was abandoned.
    #[error("response from {host} is larger than {limit} bytes")]
    ResponseTooLarge {
        /// The host that sent the response.
        host: String,
        /// The cap in bytes.
        limit: u64,
    },

    /// A declared host pattern or LAN host exception is malformed or too broad.
    #[error("invalid host pattern {pattern:?}: {reason}")]
    InvalidHostPattern {
        /// The pattern as given.
        pattern: String,
        /// Why it was rejected.
        reason: &'static str,
    },

    /// The request never reached a response.
    #[error("request to {host} failed: {source}")]
    Transport {
        /// The host the request was addressed to.
        host: String,
        /// The underlying transport error.
        #[source]
        source: HttpError,
    },

    /// The HTTP client could not be constructed.
    #[error("failed to build HTTP client: {source}")]
    Build {
        /// The cause of the failure.
        #[source]
        source: HttpError,
    },

    /// `AniList` answered 403, which means the service is unavailable.
    #[error("AniList is unavailable")]
    AniListUnavailable,

    /// The host answered 429 and no retry could complete the request.
    #[error("{host} rate limited the request; retry after {retry_after:?}")]
    RateLimited {
        /// The host that rate limited the request.
        host: String,
        /// The delay the host advertised in `Retry-After` or `X-RateLimit-Reset`.
        retry_after: Duration,
    },

    /// The host answered with an HTTP error that has no more specific variant.
    #[error("{host} returned HTTP {status}")]
    Http {
        /// The host the request was addressed to.
        host: String,
        /// The status code returned by the host.
        status: u16,
    },

    /// The request body cannot be cloned, so the request cannot be retried.
    #[error("request to {host} has a body that cannot be retried")]
    Unretryable {
        /// The host the request was addressed to.
        host: String,
    },

    /// The request could not be built.
    #[error("invalid request: {source}")]
    InvalidRequest {
        /// The cause of the failure.
        #[source]
        source: HttpError,
    },

    /// The response body could not be read.
    #[error("failed to read response: {source}")]
    Body {
        /// The cause of the failure.
        #[source]
        source: HttpError,
    },

    /// A blocking cache task failed before it could finish.
    #[error("background cache task failed: {source}")]
    Task {
        /// The cause of the failure.
        #[source]
        source: HttpError,
    },

    /// A disk-cache operation failed.
    #[error("cache operation failed at {path:?}: {source}")]
    Cache {
        /// The path involved in the failure.
        path: PathBuf,
        /// The cause of the failure.
        #[source]
        source: HttpError,
    },
}
