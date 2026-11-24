//! Request and response types owned by `net`.
//!
//! Keeping these here means the rest of the workspace never names a reqwest
//! type (ARCHITECTURE §3, rule 4).

use reqwest::header::{HeaderName, HeaderValue};

use crate::{Error, HttpError, activity::InFlight};

/// HTTP method of a request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Method {
    /// A read request.
    Get,
    /// A create or mutation request.
    Post,
    /// A replace request.
    Put,
    /// A partial-update request.
    Patch,
    /// A delete request.
    Delete,
}

impl Method {
    fn into_reqwest(self) -> reqwest::Method {
        match self {
            Self::Get => reqwest::Method::GET,
            Self::Post => reqwest::Method::POST,
            Self::Put => reqwest::Method::PUT,
            Self::Patch => reqwest::Method::PATCH,
            Self::Delete => reqwest::Method::DELETE,
        }
    }
}

/// A request to send through [`Client`](crate::Client).
#[derive(Debug, Clone)]
pub struct Request {
    method: Method,
    url: String,
    host: String,
    headers: Vec<(HeaderName, HeaderValue)>,
    body: Option<Vec<u8>>,
}

impl Request {
    /// Create a request for a URL. A malformed URL fails when the request is sent.
    #[must_use]
    pub fn new(method: Method, url: impl Into<String>) -> Self {
        let url = url.into();
        let host = reqwest::Url::parse(&url)
            .ok()
            .and_then(|parsed| parsed.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_default();
        Self {
            method,
            url,
            host,
            headers: Vec::new(),
            body: None,
        }
    }

    /// Add a header, failing if its name or value is not valid HTTP.
    ///
    /// # Errors
    ///
    /// Returns [`Error::InvalidRequest`] if the header is malformed.
    pub fn header(mut self, name: &str, value: &str) -> Result<Self, Error> {
        let name = HeaderName::from_bytes(name.as_bytes()).map_err(HttpError::new)?;
        let value = HeaderValue::from_str(value).map_err(HttpError::new)?;
        self.headers.push((name, value));
        Ok(self)
    }

    /// Set the raw request body.
    #[must_use]
    pub fn body(mut self, body: Vec<u8>) -> Self {
        self.body = Some(body);
        self
    }

    /// The target URL.
    #[must_use]
    pub fn url(&self) -> &str {
        &self.url
    }

    pub(crate) fn host(&self) -> &str {
        &self.host
    }

    pub(crate) fn into_reqwest(self) -> Result<reqwest::Request, Error> {
        let url = reqwest::Url::parse(&self.url).map_err(HttpError::new)?;
        let mut request = reqwest::Request::new(self.method.into_reqwest(), url);
        for (name, value) in self.headers {
            request.headers_mut().append(name, value);
        }
        if let Some(body) = self.body {
            *request.body_mut() = Some(body.into());
        }
        Ok(request)
    }
}

/// An HTTP response returned by [`Client`](crate::Client).
#[derive(Debug)]
pub struct Response {
    status: u16,
    inner: reqwest::Response,
    _in_flight: InFlight,
}

impl Response {
    pub(crate) fn new(inner: reqwest::Response, in_flight: InFlight) -> Self {
        let status = inner.status().as_u16();
        Self {
            status,
            inner,
            _in_flight: in_flight,
        }
    }

    /// The HTTP status code as an integer.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// Whether the status is in the 2xx range.
    #[must_use]
    pub fn is_success(&self) -> bool {
        (200..300).contains(&self.status)
    }

    /// The response's header value, if present.
    #[must_use]
    pub fn header(&self, name: &str) -> Option<&str> {
        self.inner
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
    }

    /// Read the entire body as bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Body`] if the response cannot be read.
    pub async fn bytes(self) -> Result<Vec<u8>, Error> {
        self.inner
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|source| Error::Body {
                source: HttpError::new(source),
            })
    }

    /// Read the entire body as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Body`] if the response cannot be read.
    pub async fn text(self) -> Result<String, Error> {
        self.inner.text().await.map_err(|source| Error::Body {
            source: HttpError::new(source),
        })
    }
}
