//! Errors from indexer parsing and transport.

use std::time::Duration;

use thiserror::Error;

use crate::result::Indexer;

/// A failure while building, sending or reading an indexer request.
#[derive(Debug, Clone, Error)]
#[non_exhaustive]
pub enum Error {
    /// The RSS response was not well-formed.
    #[error("invalid RSS XML: {0}")]
    Xml(String),

    /// A fixed HTML selector failed to compile.
    #[error("invalid HTML selector: {0}")]
    Selector(String),

    /// The JSON response did not have the expected shape.
    #[error("invalid JSON response: {0}")]
    Json(String),

    /// A configured base URL was not an http(s) URL with a host.
    #[error("invalid indexer base URL: {0}")]
    BaseUrl(String),

    /// The request never reached a usable response.
    #[error("request to {} failed: {message}", indexer.label())]
    Transport {
        /// The indexer that was queried.
        indexer: Indexer,
        /// What went wrong.
        message: String,
    },

    /// The indexer answered with an HTTP error.
    #[error("{} returned HTTP {status}", indexer.label())]
    Http {
        /// The indexer that answered.
        indexer: Indexer,
        /// The status code returned.
        status: u16,
    },

    /// The indexer answered 429 or 503 and no retry could complete the request.
    #[error("{} rate limited the request; retry after {retry_after:?}", indexer.label())]
    RateLimited {
        /// The indexer that answered.
        indexer: Indexer,
        /// The delay the indexer advertised, or the backoff used.
        retry_after: Duration,
    },
}
