use std::fmt;

use serde_json::{Map, Value};

/// What the API said when it refused a request.
#[derive(Clone, Debug)]
pub struct ApiError {
    /// Why the request was refused.
    pub message: String,
    /// The HTTP status.
    pub status: u16,
    /// Field errors, when the API names fields: `{"amount": ["…"]}`.
    pub errors: Map<String, Value>,
}

impl fmt::Display for ApiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for ApiError {}

/// Every failure of a request. `message()` says why, `status()` gives the HTTP status (0 when there was none).
#[derive(Clone, Debug)]
#[non_exhaustive]
pub enum Error {
    /// Missing, wrong or revoked API key, or the gateway is suspended (401).
    Authentication(ApiError),
    /// Not found, or it belongs to another gateway (404).
    NotFound(ApiError),
    /// The request was refused; `errors` names the fields (422).
    Validation(ApiError),
    /// Too many requests (429). Retried automatically, then returned.
    RateLimit(ApiError),
    /// Anything else the API refused, with its status.
    Api(ApiError),
    /// Coino Live could not be reached (network, DNS, timeout). Retried automatically, then returned.
    Connection(String),
    /// The client was given something it cannot use: an API key or base URL in the wrong form, or params that do
    /// not encode as JSON.
    InvalidArgument(String),
}

impl Error {
    pub(crate) fn from_status(status: u16, message: String, errors: Map<String, Value>) -> Self {
        let error = ApiError { message, status, errors };
        match status {
            401 => Error::Authentication(error),
            404 => Error::NotFound(error),
            422 => Error::Validation(error),
            429 => Error::RateLimit(error),
            _ => Error::Api(error),
        }
    }

    /// The API's answer, for every error that came from the API.
    pub fn api_error(&self) -> Option<&ApiError> {
        match self {
            Error::Authentication(e) | Error::NotFound(e) | Error::Validation(e) | Error::RateLimit(e) | Error::Api(e) => Some(e),
            Error::Connection(_) | Error::InvalidArgument(_) => None,
        }
    }

    /// Why the request failed.
    pub fn message(&self) -> &str {
        match self {
            Error::Connection(message) | Error::InvalidArgument(message) => message,
            Error::Authentication(e) | Error::NotFound(e) | Error::Validation(e) | Error::RateLimit(e) | Error::Api(e) => &e.message,
        }
    }

    /// The HTTP status, or 0 when the API did not answer.
    pub fn status(&self) -> u16 {
        self.api_error().map_or(0, |e| e.status)
    }

    /// Field errors, when the API names fields: `{"amount": ["…"]}`.
    pub fn errors(&self) -> Option<&Map<String, Value>> {
        self.api_error().map(|e| &e.errors)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for Error {}

/// A webhook whose signature is missing, wrong or too old: refuse it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignatureVerificationError {
    message: String,
}

impl SignatureVerificationError {
    pub(crate) fn new(message: &str) -> Self {
        SignatureVerificationError { message: message.to_string() }
    }

    /// Why the webhook was refused.
    pub fn message(&self) -> &str {
        &self.message
    }
}

impl fmt::Display for SignatureVerificationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SignatureVerificationError {}
