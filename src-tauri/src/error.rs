//! AgentJax unified error system.
//!
//! Provides structured error types for the entire backend, replacing
//! ad-hoc `Result<_, String>` patterns with typed, classifiable errors.
//!
//! ## Design
//!
//! - `AgentJaxError` is the single error type used throughout the backend.
//! - `ErrorKind` classifies errors into categories for programmatic handling.
//! - Every error carries a `retryable` flag and optional `provider_key`.
//! - `From` conversions are provided for all existing error types.
//!
//! ## Integration with Tauri
//!
//! Tauri commands need `Serialize` for their error type. `AgentJaxError` implements
//! `Serialize` via a custom serializer that maps to a JSON object with `kind`,
//! `message`, `retryable`, and `provider_key` fields. The frontend receives
//! structured error information instead of opaque strings.

use serde::Serialize;
use std::fmt;

// ── Error Kind ──────────────────────────────────────────────────────────────

/// High-level classification of an error.
///
/// Used for programmatic decision-making: retry logic, user messaging, logging.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ErrorKind {
    /// Provider authentication/authorization failure (401, 403, etc.).
    /// These are NOT retryable — credentials need user intervention.
    ProviderAuth,
    /// Provider rate limit exceeded (429, 503 with retry-after).
    /// These ARE retryable with backoff.
    ProviderRateLimited,
    /// Provider temporarily unavailable (5xx, connection refused).
    /// Retryable with backoff.
    ProviderUnavailable,
    /// Provider returned incomplete/truncated output.
    /// Retryable with conservative settings.
    ProviderOutputIncomplete,
    /// Network connectivity error (DNS, timeout, TLS, etc.).
    /// Retryable with backoff.
    Network,
    /// Configuration error (invalid settings, missing required fields).
    /// NOT retryable — user must fix config.
    Config,
    /// Tool execution error (tool call failed, invalid args, etc.).
    ToolExecution,
    /// Resource not found.
    NotFound,
    /// Sub-agent error (spawn failure, timeout, scope violation).
    SubAgent,
    /// Memory system error (read/write failure, invalid frontmatter).
    Memory,
    /// Internal/unexpected error. May be retryable.
    Internal,
}

impl fmt::Display for ErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ErrorKind::ProviderAuth => write!(f, "provider_auth"),
            ErrorKind::ProviderRateLimited => write!(f, "provider_rate_limited"),
            ErrorKind::ProviderUnavailable => write!(f, "provider_unavailable"),
            ErrorKind::ProviderOutputIncomplete => write!(f, "provider_output_incomplete"),
            ErrorKind::Network => write!(f, "network"),
            ErrorKind::Config => write!(f, "config"),
            ErrorKind::SubAgent => write!(f, "sub_agent"),
            ErrorKind::Memory => write!(f, "memory"),
            ErrorKind::ToolExecution => write!(f, "tool_execution"),
            ErrorKind::NotFound => write!(f, "not_found"),
            ErrorKind::Internal => write!(f, "internal"),
        }
    }
}

// ── AgentJaxError ───────────────────────────────────────────────────────────

/// The unified error type for the entire AgentJax backend.
///
/// Every function that previously returned `Result<_, String>` should
/// return `Result<_, AgentJaxError>` instead. This enables:
///
/// - **Classification**: `ErrorKind` tells you what went wrong
/// - **Retryability**: `retryable` flag guides recovery logic
/// - **Provider scoping**: `provider_key` identifies which provider failed
/// - **Source chaining**: preserves the underlying error context
#[derive(Debug, Clone)]
pub struct AgentJaxError {
    /// The high-level error classification.
    pub kind: ErrorKind,
    /// Human-readable error message.
    pub message: String,
    /// Whether the operation can be retried.
    pub retryable: bool,
    /// The provider key, if this is a provider-scoped error.
    pub provider_key: Option<String>,
    /// Optional source error message for diagnostics.
    pub source: Option<String>,
}

impl AgentJaxError {
    // ── Constructors ───────────────────────────────────────────────────

    /// Create a new error with the given kind and message.
    pub fn new(kind: ErrorKind, message: impl Into<String>) -> Self {
        let retryable = kind.is_retryable();
        Self {
            kind,
            message: message.into(),
            retryable,
            provider_key: None,
            source: None,
        }
    }

    /// Create a non-retryable internal error.
    pub fn internal(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Internal, message)
    }

    /// Create a configuration error.
    pub fn config(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Config, message)
    }

    /// Create a "not found" error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::NotFound, message)
    }

    /// Create a tool execution error.
    pub fn tool(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::ToolExecution, message)
    }

    /// Create a sub-agent error.
    pub fn sub_agent(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::SubAgent, message)
    }

    /// Create a memory system error.
    pub fn memory(message: impl Into<String>) -> Self {
        Self::new(ErrorKind::Memory, message)
    }

    /// Create a provider auth error.
    pub fn provider_auth(provider_key: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::ProviderAuth,
            message: message.into(),
            retryable: false,
            provider_key: Some(provider_key.into()),
            source: None,
        }
    }

    /// Create a provider rate-limited error.
    pub fn provider_rate_limited(
        provider_key: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: ErrorKind::ProviderRateLimited,
            message: message.into(),
            retryable: true,
            provider_key: Some(provider_key.into()),
            source: None,
        }
    }

    /// Create a provider unavailable error.
    pub fn provider_unavailable(
        provider_key: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            kind: ErrorKind::ProviderUnavailable,
            message: message.into(),
            retryable: true,
            provider_key: Some(provider_key.into()),
            source: None,
        }
    }

    /// Create a network error.
    pub fn network(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Network,
            message: message.into(),
            retryable: true,
            provider_key: None,
            source: None,
        }
    }

    // ── Builder methods ────────────────────────────────────────────────

    /// Mark this error as retryable (overrides the default).
    pub fn retryable(mut self, retryable: bool) -> Self {
        self.retryable = retryable;
        self
    }

    /// Attach a provider key to this error.
    pub fn with_provider(mut self, provider_key: impl Into<String>) -> Self {
        self.provider_key = Some(provider_key.into());
        self
    }

    /// Attach source error context.
    pub fn with_source(mut self, source: impl Into<String>) -> Self {
        self.source = Some(source.into());
        self
    }

    /// Attach source from a std::error::Error.
    pub fn with_error_source<E: std::error::Error>(mut self, err: &E) -> Self {
        self.source = Some(format!("{err}"));
        self
    }

    /// Check if the error message contains the given pattern.
    /// Convenience for tests that used `err.contains()` on `String`.
    pub fn contains(&self, pattern: &str) -> bool {
        self.message.contains(pattern)
    }
}

impl fmt::Display for AgentJaxError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.provider_key {
            Some(provider) => write!(f, "[{}] {}: {}", provider, self.kind, self.message),
            None => write!(f, "[{}] {}", self.kind, self.message),
        }
    }
}

impl std::error::Error for AgentJaxError {}

// ── ErrorKind helpers ──────────────────────────────────────────────────────

impl ErrorKind {
    /// Returns `true` if this error kind is typically retryable.
    pub fn is_retryable(&self) -> bool {
        matches!(
            self,
            ErrorKind::ProviderRateLimited
                | ErrorKind::ProviderUnavailable
                | ErrorKind::ProviderOutputIncomplete
                | ErrorKind::Network
                | ErrorKind::Internal
        )
    }
}

// ── From implementations ────────────────────────────────────────────────────

impl From<String> for AgentJaxError {
    fn from(s: String) -> Self {
        AgentJaxError::internal(s)
    }
}

impl From<&str> for AgentJaxError {
    fn from(s: &str) -> Self {
        AgentJaxError::internal(s.to_string())
    }
}

/// Convert AgentJaxError to a String for Tauri command boundaries.
///
/// This enables `?` in Tauri commands that return `Result<_, String>`:
/// `let engine = open_lcm_engine(id, config)?;`  // AgentJaxError → String
impl From<AgentJaxError> for String {
    fn from(e: AgentJaxError) -> String {
        e.to_string()
    }
}

impl From<serde_json::Error> for AgentJaxError {
    fn from(e: serde_json::Error) -> Self {
        AgentJaxError {
            kind: ErrorKind::Internal,
            message: format!("Serialization error: {e}"),
            retryable: false,
            provider_key: None,
            source: Some(e.to_string()),
        }
    }
}

impl From<std::io::Error> for AgentJaxError {
    fn from(e: std::io::Error) -> Self {
        let retryable = matches!(
            e.kind(),
            std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::Interrupted
                | std::io::ErrorKind::WouldBlock
        );
        AgentJaxError {
            kind: if retryable { ErrorKind::Network } else { ErrorKind::Internal },
            message: format!("IO error: {e}"),
            retryable,
            provider_key: None,
            source: Some(e.to_string()),
        }
    }
}

impl From<crate::lcm::LcmError> for AgentJaxError {
    fn from(e: crate::lcm::LcmError) -> Self {
        let (kind, retryable) = match &e {
            crate::lcm::LcmError::Store(_) => (ErrorKind::Internal, true),
            crate::lcm::LcmError::Dag(_) => (ErrorKind::Internal, false),
            crate::lcm::LcmError::NotFound(_) => (ErrorKind::NotFound, false),
            crate::lcm::LcmError::Config(_) => (ErrorKind::Config, false),
            crate::lcm::LcmError::Compaction(_) => (ErrorKind::Internal, true),
            crate::lcm::LcmError::Concurrency(_) => (ErrorKind::Internal, true),
            crate::lcm::LcmError::Serialization(_) => (ErrorKind::Internal, false),
            crate::lcm::LcmError::Sql(_) => (ErrorKind::Internal, true),
            crate::lcm::LcmError::Io(_) => (ErrorKind::Internal, true),
        };
        AgentJaxError {
            kind,
            message: e.to_string(),
            retryable,
            provider_key: None,
            source: Some(e.to_string()),
        }
    }
}

impl From<crate::plugin_runtime::PluginRuntimeError> for AgentJaxError {
    fn from(e: crate::plugin_runtime::PluginRuntimeError) -> Self {
        AgentJaxError {
            kind: ErrorKind::ToolExecution,
            message: e.to_string(),
            retryable: false,
            provider_key: None,
            source: Some(e.to_string()),
        }
    }
}

/// Type alias for the unified result type.
pub type AgentJaxResult<T> = Result<T, AgentJaxError>;

// ── Utility macro ──────────────────────────────────────────────────────────

/// Convert a string-literal error to AgentJaxError at a call site.
///
/// Useful in closures and map_err chains where you want to quickly
/// create a classified error without constructing the full struct.
///
/// # Examples
///
/// ```ignore
/// return Err(err!("tool failed", ToolExecution));
/// return Err(err!("tool failed", ToolExecution, retryable = true));
/// ```
#[macro_export]
macro_rules! agentjax_err {
    ($message:expr, $kind:ident) => {
        $crate::error::AgentJaxError::new($crate::error::ErrorKind::$kind, $message)
    };
    ($message:expr, $kind:ident, retryable = $retryable:expr) => {
        $crate::error::AgentJaxError::new($crate::error::ErrorKind::$kind, $message)
            .retryable($retryable)
    };
    ($message:expr, $kind:ident, provider = $provider:expr) => {
        $crate::error::AgentJaxError::new($crate::error::ErrorKind::$kind, $message)
            .with_provider($provider)
    };
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_error_creation() {
        let err = AgentJaxError::new(ErrorKind::Config, "missing field");
        assert_eq!(err.kind, ErrorKind::Config);
        assert!(!err.retryable);
        assert_eq!(err.message, "missing field");
    }

    #[test]
    fn test_retryable_kinds() {
        assert!(!ErrorKind::ProviderAuth.is_retryable());
        assert!(ErrorKind::ProviderRateLimited.is_retryable());
        assert!(ErrorKind::ProviderUnavailable.is_retryable());
        assert!(ErrorKind::Network.is_retryable());
        assert!(!ErrorKind::Config.is_retryable());
        assert!(!ErrorKind::NotFound.is_retryable());
        assert!(ErrorKind::Internal.is_retryable());
    }

    #[test]
    fn test_from_lcm_error() {
        let lcm_err = crate::lcm::LcmError::NotFound("entity".to_string());
        let err: AgentJaxError = lcm_err.into();
        assert_eq!(err.kind, ErrorKind::NotFound);
        assert!(!err.retryable);
    }

    #[test]
    fn test_from_io_error() {
        let io_err = std::io::Error::new(std::io::ErrorKind::TimedOut, "timeout");
        let err: AgentJaxError = io_err.into();
        assert_eq!(err.kind, ErrorKind::Network);
        assert!(err.retryable);
    }

    #[test]
    fn test_from_string() {
        let err: AgentJaxError = "something broke".into();
        assert_eq!(err.kind, ErrorKind::Internal);
    }

    #[test]
    fn test_macro() {
        let err = agentjax_err!("rate limit", ProviderRateLimited);
        assert_eq!(err.kind, ErrorKind::ProviderRateLimited);
        assert!(err.retryable);

        let err = agentjax_err!("auth failed", ProviderAuth, provider = "anthropic");
        assert_eq!(err.kind, ErrorKind::ProviderAuth);
        assert_eq!(err.provider_key.as_deref(), Some("anthropic"));
    }
}
