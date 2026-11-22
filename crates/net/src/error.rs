//! Errors returned by the shared HTTP client.

use std::time::Duration;

use thiserror::Error;

/// An opaque cause from the HTTP stack.
///
/// Its field is private so the rest of the workspace never names a reqwest
/// type (ARCHITECTURE §3, rule 4).
#[derive(Debug, Error)]
#[error("{0}")]
pub struct HttpError(String);

impl HttpError {
    pub(crate) fn new(source: impl std::fmt::Display) -> Self {
        Self(source.to_string())
    }
}

impl From<HttpError> for Error {
    fn from(source: HttpError) -> Self {
        Self::InvalidRequest { source }
    }
}

/// A failure from a request made through [`Client`](crate::Client).
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The request never reached a response.
    #[error("request to {host} failed: {source}")]
    Transport {
        /// The host the request was addressed to.
        host: String,
        /// The underlying transport error.
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
}
