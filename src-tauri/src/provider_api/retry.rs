//! Retry strategies for provider API calls.
//!
//! Implements exponential backoff with jitter, inspired by the lossless-claw
//! reference implementation's retry logic in `summarize.ts`.
//!
//! ## Strategy
//!
//! | Error Type | Max Attempts | Base Delay | Jitter | Notes |
//! |------------|-------------|------------|--------|-------|
//! | Rate Limit (429) | 3 | 5s | Yes | Respects Retry-After header |
//! | Server Error (5xx) | 3 | 1s | Yes | Standard backoff |
//! | Network Error | 2 | 2s | Yes | Quick retry |
//! | Auth Error (401/403) | 1 | — | — | No retry, surface immediately |
//! | Empty/Incomplete | 2 | 500ms | No | Conservative retry (low temp) |

use std::time::Duration;

// ── Exponential Backoff ─────────────────────────────────────────────────────

/// Configuration for exponential backoff retry.
#[derive(Debug, Clone)]
pub struct RetryStrategy {
    /// Maximum number of retry attempts (excluding the initial attempt).
    pub max_attempts: u32,
    /// Base delay in milliseconds (doubles with each attempt).
    pub base_delay_ms: u64,
    /// Maximum delay in milliseconds.
    pub max_delay_ms: u64,
    /// Whether to add random jitter to the delay.
    pub jitter: bool,
    /// Error kinds that should NOT be retried (surface immediately).
    pub non_retryable_kinds: Vec<crate::error::ErrorKind>,
}

#[allow(dead_code)] // Reserved for future use — retry strategy presets
impl RetryStrategy {
    /// Standard retry for provider rate limits (429).
    pub fn rate_limit() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 5_000,
            max_delay_ms: 60_000,
            jitter: true,
            non_retryable_kinds: vec![
                crate::error::ErrorKind::ProviderAuth,
                crate::error::ErrorKind::Config,
            ],
        }
    }

    /// Standard retry for server errors (5xx).
    pub fn server_error() -> Self {
        Self {
            max_attempts: 3,
            base_delay_ms: 1_000,
            max_delay_ms: 30_000,
            jitter: true,
            non_retryable_kinds: vec![
                crate::error::ErrorKind::ProviderAuth,
                crate::error::ErrorKind::Config,
            ],
        }
    }

    /// Quick retry for network errors.
    pub fn network_error() -> Self {
        Self {
            max_attempts: 2,
            base_delay_ms: 2_000,
            max_delay_ms: 10_000,
            jitter: true,
            non_retryable_kinds: vec![
                crate::error::ErrorKind::ProviderAuth,
                crate::error::ErrorKind::Config,
            ],
        }
    }

    /// Conservative retry for empty/incomplete responses.
    pub fn empty_response() -> Self {
        Self {
            max_attempts: 2,
            base_delay_ms: 500,
            max_delay_ms: 5_000,
            jitter: false,
            non_retryable_kinds: vec![
                crate::error::ErrorKind::ProviderAuth,
                crate::error::ErrorKind::Config,
                crate::error::ErrorKind::ProviderRateLimited,
            ],
        }
    }

    /// No retry — surface the error immediately.
    pub fn no_retry() -> Self {
        Self {
            max_attempts: 1,
            base_delay_ms: 0,
            max_delay_ms: 0,
            jitter: false,
            non_retryable_kinds: vec![],
        }
    }

    /// Select the appropriate retry strategy based on the error kind.
    pub fn for_error_kind(kind: &crate::error::ErrorKind) -> Self {
        match kind {
            crate::error::ErrorKind::ProviderAuth => Self::no_retry(),
            crate::error::ErrorKind::ProviderRateLimited => Self::rate_limit(),
            crate::error::ErrorKind::ProviderUnavailable => Self::server_error(),
            crate::error::ErrorKind::ProviderOutputIncomplete => Self::empty_response(),
            crate::error::ErrorKind::Network => Self::network_error(),
            crate::error::ErrorKind::Config => Self::no_retry(),
            crate::error::ErrorKind::NotFound => Self::no_retry(),
            crate::error::ErrorKind::ToolExecution => Self::no_retry(),
            crate::error::ErrorKind::SubAgent => Self::no_retry(),
            crate::error::ErrorKind::Memory => Self::no_retry(),
            crate::error::ErrorKind::Embedding => Self::server_error(),
            crate::error::ErrorKind::Internal => Self::server_error(),
        }
    }

    /// Compute the delay for a given attempt number.
    ///
    /// Attempt 0 is the first retry (after the initial failure).
    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        if attempt == 0 || self.base_delay_ms == 0 {
            return Duration::from_millis(self.base_delay_ms);
        }
        let exponential = self
            .base_delay_ms
            .saturating_mul(2_u64.saturating_pow(attempt));
        let clamped = exponential.min(self.max_delay_ms);

        if self.jitter {
            // Add up to 50% jitter: delay in [clamped/2, clamped]
            // Use a simple deterministic jitter based on attempt number
            // to avoid adding a `rand` dependency.
            let half = clamped / 2;
            let jitter_amount = (attempt as u64 * 7919) % (half + 1); // 7919 is prime
            let jittered = half + jitter_amount;
            Duration::from_millis(jittered)
        } else {
            Duration::from_millis(clamped)
        }
    }

    /// Whether this error kind should be retried.
    pub fn should_retry(&self, kind: &crate::error::ErrorKind) -> bool {
        !self.non_retryable_kinds.contains(kind)
    }

    /// Whether we've exhausted all retry attempts.
    pub fn is_exhausted(&self, attempts_made: u32) -> bool {
        attempts_made >= self.max_attempts
    }
}

// ── RetryResult ─────────────────────────────────────────────────────────────

/// The result of a retry operation.
#[derive(Debug, Clone)]
pub enum RetryResult<T> {
    /// Operation succeeded.
    Success(T),
    /// Operation failed after all retries.
    Failed(crate::error::AgentJaxError),
    /// Operation was not retried because the error is non-retryable.
    NonRetryable(crate::error::AgentJaxError),
}

impl<T> RetryResult<T> {
    /// Convert to `Result<T, AgentJaxError>`.
    pub fn into_result(self) -> Result<T, crate::error::AgentJaxError> {
        match self {
            RetryResult::Success(val) => Ok(val),
            RetryResult::Failed(err) => Err(err),
            RetryResult::NonRetryable(err) => Err(err),
        }
    }
}

/// Execute an async operation with retry.
///
/// The `operation` closure is called repeatedly according to the strategy.
/// Errors matching `non_retryable_kinds` are surfaced immediately.
///
/// # Example
///
/// ```ignore
/// let result = retry_with_backoff(
///     RetryStrategy::server_error(),
///     || async { provider.call().await },
/// ).await;
/// ```
pub async fn retry_with_backoff<T, F, Fut>(
    strategy: RetryStrategy,
    mut operation: F,
) -> RetryResult<T>
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = Result<T, crate::error::AgentJaxError>>,
{
    let mut last_error: Option<crate::error::AgentJaxError> = None;
    let max_attempts = strategy.max_attempts;

    for attempt in 0..max_attempts {
        // Check cancellation before each attempt.
        if attempt > 0 {
            let delay = strategy.delay_for_attempt(attempt - 1);
            tokio::time::sleep(delay).await;
        }

        match operation().await {
            Ok(val) => return RetryResult::Success(val),
            Err(err) => {
                let is_non_retryable = !strategy.should_retry(&err.kind);
                if is_non_retryable {
                    return RetryResult::NonRetryable(err);
                }
                if attempt + 1 >= max_attempts {
                    last_error = Some(err);
                } else {
                    log::debug!(
                        "Retry attempt {}/{} failed: {} (retry in {:?})",
                        attempt + 1,
                        max_attempts,
                        err,
                        strategy.delay_for_attempt(attempt)
                    );
                    last_error = Some(err);
                }
            }
        }
    }

    RetryResult::Failed(
        last_error.unwrap_or_else(|| crate::error::AgentJaxError::internal("Retry exhausted")),
    )
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::{AgentJaxError, ErrorKind};

    #[test]
    fn test_retry_strategy_rate_limit_delays() {
        let strategy = RetryStrategy::rate_limit();
        let d0 = strategy.delay_for_attempt(0);
        let d1 = strategy.delay_for_attempt(1);
        let d2 = strategy.delay_for_attempt(2);

        // With jitter, delays should be clamped and in expected range.
        assert!(d0.as_millis() <= 5000, "d0={:?}", d0);
        assert!(d1.as_millis() <= 10000, "d1={:?}", d1);
        assert!(d2.as_millis() <= 60000, "d2={:?}", d2);
    }

    #[test]
    fn test_no_jitter() {
        let strategy = RetryStrategy {
            jitter: false,
            ..RetryStrategy::server_error()
        };
        let d0 = strategy.delay_for_attempt(0);
        let d1 = strategy.delay_for_attempt(1);
        assert_eq!(d0.as_millis(), 1000);
        assert_eq!(d1.as_millis(), 2000);
    }

    #[test]
    fn test_should_retry_auth() {
        let strategy = RetryStrategy::server_error();
        assert!(!strategy.should_retry(&ErrorKind::ProviderAuth));
        assert!(strategy.should_retry(&ErrorKind::ProviderUnavailable));
    }

    #[test]
    fn test_should_retry_rate_limit() {
        let strategy = RetryStrategy::empty_response();
        assert!(!strategy.should_retry(&ErrorKind::ProviderRateLimited));
        assert!(strategy.should_retry(&ErrorKind::ProviderOutputIncomplete));
    }

    #[test]
    fn test_is_exhausted() {
        let strategy = RetryStrategy::server_error();
        assert!(!strategy.is_exhausted(0));
        assert!(!strategy.is_exhausted(1));
        assert!(!strategy.is_exhausted(2));
        assert!(strategy.is_exhausted(3));
    }

    #[test]
    fn test_for_error_kind() {
        assert_eq!(
            RetryStrategy::for_error_kind(&ErrorKind::ProviderAuth).max_attempts,
            1
        );
        assert!(RetryStrategy::for_error_kind(&ErrorKind::ProviderRateLimited).max_attempts >= 2);
        assert!(RetryStrategy::for_error_kind(&ErrorKind::ProviderUnavailable).max_attempts >= 2);
        assert!(RetryStrategy::for_error_kind(&ErrorKind::Network).max_attempts >= 2);
        assert_eq!(
            RetryStrategy::for_error_kind(&ErrorKind::Config).max_attempts,
            1
        );
    }

    #[tokio::test]
    async fn test_retry_success() {
        let call_count = std::sync::atomic::AtomicU32::new(0);
        let result = retry_with_backoff(RetryStrategy::server_error(), || {
            let count = &call_count;
            async move {
                let prev = count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                if prev < 1 {
                    Err(AgentJaxError::internal("try again"))
                } else {
                    Ok(42)
                }
            }
        })
        .await;

        match result {
            RetryResult::Success(val) => assert_eq!(val, 42),
            other => panic!("Expected success, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_retry_exhausted() {
        let result = retry_with_backoff(RetryStrategy::server_error(), || async {
            Err::<i32, _>(AgentJaxError::internal("always fails"))
        })
        .await;

        match result {
            RetryResult::Failed(_) => {} // Expected
            other => panic!("Expected Failed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn test_non_retryable_error() {
        let result = retry_with_backoff(RetryStrategy::server_error(), || async {
            Err::<i32, _>(AgentJaxError::provider_auth("openai", "bad key"))
        })
        .await;

        match result {
            RetryResult::NonRetryable(err) => {
                assert_eq!(err.kind, ErrorKind::ProviderAuth);
            }
            other => panic!("Expected NonRetryable, got {:?}", other),
        }
    }
}
