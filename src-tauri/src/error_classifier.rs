//! Provider error classification.
//!
//! Classifies raw provider errors (HTTP responses, network errors, etc.)
//! into `ErrorKind` categories. Inspired by the lossless-claw reference
//! implementation's `extractAuthFailureStatusCode` and `detectProviderAuthError`.
//!
//! # Note
//!
//! Dead-code warnings are suppressed because this module is built before
//! the provider_api migration. The types and functions here will be fully
//! used once provider_api is migrated to `AgentJaxError`.

#![allow(dead_code)]
//!
//! ## Classification Rules
//!
//! | HTTP Status | Classification | Retryable | Action |
//! |-------------|---------------|-----------|--------|
//! | 401, 403 | ProviderAuth | No | Surface to user, don't retry |
//! | 429 | ProviderRateLimited | Yes | Backoff + retry |
//! | 408, 5xx (except 501) | ProviderUnavailable | Yes | Backoff + retry |
//! | 400, 404, 422 | Config | No | Surface to user |
//! | Connection refused/timeout | Network | Yes | Backoff + retry |
//! | TLS/SSL errors | Network | Yes | Backoff + retry |
//! | Empty/incomplete output | ProviderOutputIncomplete | Yes | Retry with conservative settings |

use crate::error::{AgentJaxError, ErrorKind};
use std::time::Duration;

// Allow unused for now — will be fully used when provider_api is migrated.
#[allow(dead_code)]

/// Represents a raw provider error from an API call.
#[derive(Debug, Clone)]
pub struct RawProviderError {
    /// HTTP status code, if available.
    pub status_code: Option<u16>,
    /// The provider's error code string, if available.
    pub error_code: Option<String>,
    /// The provider's error message.
    pub message: String,
    /// The provider key (e.g., "openai", "anthropic").
    pub provider_key: Option<String>,
    /// Retry-After header value, if present.
    pub retry_after: Option<Duration>,
    /// Whether the response body was empty.
    pub is_empty_response: bool,
    /// Whether the response was incomplete/truncated.
    pub is_incomplete: bool,
    /// The raw error type for network errors.
    pub network_error_kind: Option<NetworkErrorKind>,
}

/// Classification of network-level errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NetworkErrorKind {
    Timeout,
    ConnectionRefused,
    ConnectionReset,
    DnsLookupFailed,
    TlsError,
    Other,
}

impl RawProviderError {
    /// Create from an HTTP status code and message.
    pub fn from_http_status(
        status: u16,
        message: impl Into<String>,
        provider_key: Option<impl Into<String>>,
    ) -> Self {
        Self {
            status_code: Some(status),
            error_code: None,
            message: message.into(),
            provider_key: provider_key.map(|s| s.into()),
            retry_after: None,
            is_empty_response: false,
            is_incomplete: false,
            network_error_kind: None,
        }
    }

    /// Create from a network error.
    pub fn from_network_error(
        kind: NetworkErrorKind,
        message: impl Into<String>,
    ) -> Self {
        Self {
            status_code: None,
            error_code: None,
            message: message.into(),
            provider_key: None,
            retry_after: None,
            is_empty_response: false,
            is_incomplete: false,
            network_error_kind: Some(kind),
        }
    }

    /// Mark the response as empty.
    pub fn with_empty_response(mut self) -> Self {
        self.is_empty_response = true;
        self
    }

    /// Mark the response as incomplete.
    pub fn with_incomplete(mut self) -> Self {
        self.is_incomplete = true;
        self
    }

    /// Set the Retry-After duration.
    pub fn with_retry_after(mut self, duration: Duration) -> Self {
        self.retry_after = Some(duration);
        self
    }

    /// Set the provider key.
    pub fn with_provider_key(mut self, key: impl Into<String>) -> Self {
        self.provider_key = Some(key.into());
        self
    }

    /// Classify this raw error into an `AgentJaxError`.
    pub fn classify(self) -> AgentJaxError {
        let provider = self.provider_key.clone().unwrap_or_default();
        let msg = self.message.clone();

        // Network errors.
        if let Some(net_kind) = self.network_error_kind {
            return match net_kind {
                NetworkErrorKind::Timeout => AgentJaxError::network(format!(
                    "Connection timed out: {msg}"
                )).with_provider(&provider),
                NetworkErrorKind::ConnectionRefused => AgentJaxError::network(format!(
                    "Connection refused: {msg}"
                )).with_provider(&provider),
                NetworkErrorKind::ConnectionReset => AgentJaxError::network(format!(
                    "Connection reset: {msg}"
                )).with_provider(&provider),
                NetworkErrorKind::DnsLookupFailed => AgentJaxError::network(format!(
                    "DNS lookup failed: {msg}"
                )).with_provider(&provider),
                NetworkErrorKind::TlsError => AgentJaxError::network(format!(
                    "TLS/SSL error: {msg}"
                )).with_provider(&provider),
                NetworkErrorKind::Other => AgentJaxError::network(format!(
                    "Network error: {msg}"
                )).with_provider(&provider),
            };
        }

        // Incomplete/empty output.
        if self.is_incomplete {
            return AgentJaxError {
                kind: ErrorKind::ProviderOutputIncomplete,
                message: format!("Provider returned incomplete output: {msg}"),
                retryable: true,
                provider_key: self.provider_key,
                source: None,
            };
        }

        if self.is_empty_response {
            return AgentJaxError {
                kind: ErrorKind::ProviderOutputIncomplete,
                message: format!("Provider returned empty response: {msg}"),
                retryable: true,
                provider_key: self.provider_key,
                source: None,
            };
        }

        // Classify by HTTP status code.
        match self.status_code {
            Some(401) | Some(403) => AgentJaxError {
                kind: ErrorKind::ProviderAuth,
                message: format!("Authentication failed ({status}): {msg}", status = self.status_code.unwrap()),
                retryable: false,
                provider_key: self.provider_key,
                source: None,
            },

            Some(429) => {
                let retry_msg = match &self.retry_after {
                    Some(d) => format!("Rate limited. Retry after {d:?}: {msg}"),
                    None => format!("Rate limited: {msg}"),
                };
                AgentJaxError {
                    kind: ErrorKind::ProviderRateLimited,
                    message: retry_msg,
                    retryable: true,
                    provider_key: self.provider_key,
                    source: None,
                }
            }

            Some(408) | Some(500) | Some(502) | Some(503) | Some(504) => AgentJaxError {
                kind: ErrorKind::ProviderUnavailable,
                message: format!("Provider unavailable ({status}): {msg}", status = self.status_code.unwrap()),
                retryable: true,
                provider_key: self.provider_key,
                source: None,
            },

            Some(400) | Some(404) | Some(422) => AgentJaxError {
                kind: ErrorKind::Config,
                message: format!("Request rejected ({status}): {msg}", status = self.status_code.unwrap()),
                retryable: false,
                provider_key: self.provider_key,
                source: None,
            },

            // Unknown status code — treat as internal.
            Some(code) => AgentJaxError {
                kind: ErrorKind::Internal,
                message: format!("Unexpected HTTP {code}: {msg}"),
                retryable: true,
                provider_key: self.provider_key,
                source: None,
            },

            // No status code — generic error.
            None => AgentJaxError::internal(msg),
        }
    }
}

/// Classify a `reqwest` error into an `AgentJaxError`.
pub fn classify_reqwest_error(err: &reqwest::Error, provider_key: Option<&str>) -> AgentJaxError {
    if err.is_timeout() {
        return RawProviderError::from_network_error(
            NetworkErrorKind::Timeout,
            err.to_string(),
        )
        .with_provider_key(provider_key.unwrap_or("unknown"))
        .classify();
    }
    if err.is_connect() {
        return RawProviderError::from_network_error(
            NetworkErrorKind::ConnectionRefused,
            err.to_string(),
        )
        .with_provider_key(provider_key.unwrap_or("unknown"))
        .classify();
    }
    if let Some(status) = err.status() {
        let msg = err.to_string();
        return RawProviderError::from_http_status(
            status.as_u16(),
            msg,
            provider_key,
        )
        .classify();
    }
    AgentJaxError::network(format!("Request failed: {err}"))
        .with_provider(provider_key.unwrap_or("unknown"))
}

/// Classify from standard HTTP status and body.
pub fn classify_http_error(
    status: u16,
    body: &str,
    provider_key: Option<&str>,
    retry_after: Option<Duration>,
) -> AgentJaxError {
    let mut err = RawProviderError::from_http_status(status, body, provider_key);
    if let Some(d) = retry_after {
        err = err.with_retry_after(d);
    }
    err.classify()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorKind;

    #[test]
    fn test_classify_401() {
        let err = RawProviderError::from_http_status(401, "Invalid API key", Some("openai"))
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderAuth);
        assert!(!err.retryable);
        assert_eq!(err.provider_key.as_deref(), Some("openai"));
    }

    #[test]
    fn test_classify_403() {
        let err = RawProviderError::from_http_status(403, "Forbidden", Some("anthropic"))
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderAuth);
        assert!(!err.retryable);
    }

    #[test]
    fn test_classify_429() {
        let err = RawProviderError::from_http_status(429, "Too many requests", Some("openai"))
            .with_retry_after(std::time::Duration::from_secs(30))
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderRateLimited);
        assert!(err.retryable);
    }

    #[test]
    fn test_classify_503() {
        let err = RawProviderError::from_http_status(503, "Service unavailable", Some("anthropic"))
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderUnavailable);
        assert!(err.retryable);
    }

    #[test]
    fn test_classify_500() {
        let err = RawProviderError::from_http_status(500, "Internal error", None::<&str>)
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderUnavailable);
        assert!(err.retryable);
    }

    #[test]
    fn test_classify_400() {
        let err = RawProviderError::from_http_status(400, "Bad request", None::<&str>)
            .classify();
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(!err.retryable);
    }

    #[test]
    fn test_classify_timeout() {
        let err = RawProviderError::from_network_error(
            NetworkErrorKind::Timeout,
            "connection timed out after 30s",
        )
        .with_provider_key("openai")
        .classify();
        assert_eq!(err.kind, ErrorKind::Network);
        assert!(err.retryable);
    }

    #[test]
    fn test_classify_incomplete() {
        let err = RawProviderError::from_http_status(200, "OK", None::<&str>)
            .with_incomplete()
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderOutputIncomplete);
        assert!(err.retryable);
    }

    #[test]
    fn test_classify_empty() {
        let err = RawProviderError::from_http_status(200, "", None::<&str>)
            .with_empty_response()
            .classify();
        assert_eq!(err.kind, ErrorKind::ProviderOutputIncomplete);
        assert!(err.retryable);
    }

    #[test]
    fn test_classify_unknown_status() {
        let err = RawProviderError::from_http_status(499, "Unknown", None::<&str>)
            .classify();
        assert_eq!(err.kind, ErrorKind::Internal);
        assert!(err.retryable);
    }

    #[test]
    fn test_http_error_helper() {
        let err = classify_http_error(429, "rate limit", Some("openai"), Some(Duration::from_secs(10)));
        assert_eq!(err.kind, ErrorKind::ProviderRateLimited);
    }
}
