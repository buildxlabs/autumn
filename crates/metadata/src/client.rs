//! The AniList client: posts `cynic` operations through `net`.

use cynic::Operation;
use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::Error;

/// AniList's GraphQL endpoint.
pub const ANILIST_ENDPOINT: &str = "https://graphql.anilist.co";

/// An unauthenticated AniList client. Cloning shares the HTTP client.
#[derive(Debug, Clone)]
pub struct AniList {
    http: net::Client,
    endpoint: String,
}

impl AniList {
    /// A client for AniList's public endpoint.
    ///
    /// `http` should have AniList's limiter registered, as
    /// [`net::Client::new`] does.
    #[must_use]
    pub fn new(http: net::Client) -> Self {
        Self::with_endpoint(http, ANILIST_ENDPOINT)
    }

    /// A client using `net`'s shared transport and AniList limiter.
    ///
    /// Callers that only need the public catalog can use this instead of
    /// depending on `net` themselves.
    ///
    /// # Errors
    ///
    /// [`net::Error::Build`] when the HTTP client cannot be initialized.
    pub fn with_default_client() -> Result<Self, Error> {
        Ok(Self::new(net::Client::new()?))
    }

    /// A client for another endpoint, such as a loopback mock in tests.
    #[must_use]
    pub fn with_endpoint(http: net::Client, endpoint: impl Into<String>) -> Self {
        Self {
            http,
            endpoint: endpoint.into(),
        }
    }

    /// Posts a query and returns its `data`.
    ///
    /// GraphQL errors on an HTTP 200 become [`Error::Api`]. `net` drops the
    /// body of any other HTTP error, so an HTTP 400 (an invalid query or
    /// filter value) arrives as [`Error::Net`] without AniList's message.
    pub(crate) async fn query<Q, V>(&self, operation: Operation<Q, V>) -> Result<Q, Error>
    where
        Q: DeserializeOwned,
        V: Serialize,
    {
        let body = serde_json::to_vec(&operation).map_err(Error::Encode)?;
        let request = net::Request::new(net::Method::Post, &self.endpoint)
            .header("Content-Type", "application/json")?
            .header("Accept", "application/json")?
            .body(body);
        let response = self.http.read(request).await?;
        let bytes = response.bytes().await?;
        let envelope: Envelope<Q> = serde_json::from_slice(&bytes).map_err(Error::Decode)?;
        if let Some(error) = envelope.errors.into_iter().next() {
            return Err(Error::Api {
                status: error.status,
                message: error.message,
            });
        }
        envelope.data.ok_or_else(|| Error::Api {
            status: None,
            message: "response has no data".to_owned(),
        })
    }
}

/// A GraphQL response. AniList puts the status at the top of each error,
/// which `cynic::GraphQlResponse` drops, so this reads it directly.
#[derive(Deserialize)]
struct Envelope<T> {
    data: Option<T>,
    #[serde(default)]
    errors: Vec<ApiError>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
    status: Option<u16>,
}
