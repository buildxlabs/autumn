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
    max_response_bytes: Option<u64>,
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
            max_response_bytes: None,
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

    /// Cap the response body; reading more fails with
    /// [`Error::ResponseTooLarge`]. Extension requests are always capped at
    /// 4 MiB, so a larger value has no effect there.
    #[must_use]
    pub fn max_response_bytes(mut self, limit: u64) -> Self {
        self.max_response_bytes = Some(limit);
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

    pub(crate) fn body_len(&self) -> usize {
        self.body.as_ref().map_or(0, Vec::len)
    }

    pub(crate) fn response_limit(&self) -> Option<u64> {
        self.max_response_bytes
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
    limit: Option<u64>,
    _in_flight: InFlight,
}

impl Response {
    pub(crate) fn new(inner: reqwest::Response, in_flight: InFlight, limit: Option<u64>) -> Self {
        let status = inner.status().as_u16();
        Self {
            status,
            inner,
            limit,
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

    /// Every value of a repeated header, such as `Set-Cookie`, in order.
    #[must_use]
    pub fn header_values(&self, name: &str) -> Vec<&str> {
        self.inner
            .headers()
            .get_all(name)
            .iter()
            .filter_map(|value| value.to_str().ok())
            .collect()
    }

    /// Read the entire body as bytes.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Body`] if the response cannot be read, and
    /// [`Error::ResponseTooLarge`] as soon as it outgrows the request's cap.
    pub async fn bytes(mut self) -> Result<Vec<u8>, Error> {
        let body_error = |source: reqwest::Error| Error::Body {
            source: HttpError::new(source.without_url()),
        };
        let Some(limit) = self.limit else {
            return self
                .inner
                .bytes()
                .await
                .map(|bytes| bytes.to_vec())
                .map_err(body_error);
        };
        let too_large = |inner: &reqwest::Response| Error::ResponseTooLarge {
            host: inner.url().host_str().unwrap_or_default().to_owned(),
            limit,
        };
        if self
            .inner
            .content_length()
            .is_some_and(|length| length > limit)
        {
            return Err(too_large(&self.inner));
        }
        let mut body = Vec::new();
        while let Some(chunk) = self.inner.chunk().await.map_err(body_error)? {
            let length = u64::try_from(body.len() + chunk.len()).unwrap_or(u64::MAX);
            if length > limit {
                return Err(too_large(&self.inner));
            }
            body.extend_from_slice(&chunk);
        }
        Ok(body)
    }

    /// Read the entire body as UTF-8 text.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Body`] if the response cannot be read, and
    /// [`Error::ResponseTooLarge`] as soon as it outgrows the request's cap.
    pub async fn text(self) -> Result<String, Error> {
        if self.limit.is_some() {
            let bytes = self.bytes().await?;
            return Ok(String::from_utf8_lossy(&bytes).into_owned());
        }
        self.inner.text().await.map_err(|source| Error::Body {
            source: HttpError::new(source),
        })
    }
}
