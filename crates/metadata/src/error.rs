//! Errors returned by the AniList client.

use thiserror::Error;

/// A failed AniList request.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum Error {
    /// The request failed in `net`: a transport error, an HTTP error, a rate
    /// limit ([`net::Error::RateLimited`]) or an outage
    /// ([`net::Error::AniListUnavailable`]).
    #[error(transparent)]
    Net(#[from] net::Error),

    /// AniList has no media with the requested id ([`crate::AniList::media`]).
    #[error("AniList has no such media")]
    NotFound,

    /// AniList answered with an invalid media id where a single media was
    /// requested. List items with invalid ids are dropped instead.
    #[error("AniList returned media with an invalid id")]
    InvalidMediaId,

    /// The token did not identify an account.
    #[error("AniList did not return an account for this token")]
    Unauthorized,

    /// AniList answered with a GraphQL error.
    #[error("AniList returned an error: {message}")]
    Api {
        /// The HTTP-like status AniList puts in `errors[].status`, if any.
        status: Option<u16>,
        /// AniList's error message.
        message: String,
    },

    /// The query could not be encoded as JSON.
    #[error("failed to encode the AniList query: {0}")]
    Encode(#[source] serde_json::Error),

    /// The response did not match the query's shape.
    #[error("failed to decode the AniList response: {0}")]
    Decode(#[source] serde_json::Error),

    /// The latest anime-offline-database release has no asset named `{0}`.
    #[error("the latest anime-offline-database release has no asset named {0:?}")]
    MissingAsset(String),
}

impl Error {
    /// Whether AniList rejected the token rather than failing for another
    /// reason. All three shapes occur: a missing viewer, a GraphQL 401 and a
    /// bare HTTP 401.
    #[must_use]
    pub fn is_unauthorized(&self) -> bool {
        matches!(
            self,
            Self::Unauthorized
                | Self::Api {
                    status: Some(401),
                    ..
                }
                | Self::Net(net::Error::Http { status: 401, .. })
        )
    }
}
