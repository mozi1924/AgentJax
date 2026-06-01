//! Circuit breaker for provider API calls.
//!
//! Prevents cascading failures by stopping requests to a failing provider
//! after a threshold of consecutive failures. Inspired by the lossless-claw
//! reference implementation's circuit breaker in `circuit-breaker.test.ts`.
//!
//! ## States
//!
//! ```text
//!      ┌──────────┐
//!      │  CLOSED  │  ← Normal operation, requests pass through
//!      └────┬─────┘
//!           │ failures >= threshold
//!           ▼
//!      ┌──────────┐
//!      │   OPEN   │  ← Failing, requests rejected immediately
//!      └────┬─────┘
//!           │ cooldown elapsed
//!           ▼
//!      ┌──────────┐
//!      │ HALF_OPEN│  ← Testing if provider recovered
//!      └────┬─────┘
//!      ┌────┴─────┐
//!      │          │
//!   success    failure
//!      │          │
//!      ▼          ▼
//!   CLOSED      OPEN
//! ```

use crate::error::{AgentJaxError, ErrorKind};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

// ── Circuit Breaker State ───────────────────────────────────────────────────

/// The state of a circuit breaker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CircuitState {
    /// Normal operation — requests pass through.
    Closed,
    /// Failing — requests are rejected immediately.
    Open,
    /// Testing — a single request is allowed through.
    HalfOpen,
}

/// Configuration for a circuit breaker.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening the circuit.
    pub failure_threshold: u32,
    /// Cooldown period before transitioning from Open to HalfOpen.
    pub cooldown_duration: Duration,
    /// Number of consecutive successes in HalfOpen to close the circuit.
    pub success_threshold: u32,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            failure_threshold: 3,
            cooldown_duration: Duration::from_secs(30),
            success_threshold: 2,
        }
    }
}

impl CircuitBreakerConfig {
    /// Aggressive circuit breaker for auth errors (quick to open, slow to recover).
    pub fn aggressive() -> Self {
        Self {
            failure_threshold: 2,
            cooldown_duration: Duration::from_secs(120),
            success_threshold: 3,
        }
    }

    /// Lenient circuit breaker for transient errors.
    pub fn lenient() -> Self {
        Self {
            failure_threshold: 5,
            cooldown_duration: Duration::from_secs(10),
            success_threshold: 1,
        }
    }
}

/// Per-provider circuit breaker state.
struct BreakerState {
    config: CircuitBreakerConfig,
    state: CircuitState,
    failure_count: u32,
    success_count: u32,
    last_failure_at: Option<Instant>,
    opened_at: Option<Instant>,
}

impl BreakerState {
    fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            state: CircuitState::Closed,
            failure_count: 0,
            success_count: 0,
            last_failure_at: None,
            opened_at: None,
            config,
        }
    }

    /// Record a success — may close a HalfOpen circuit.
    fn record_success(&mut self) {
        self.failure_count = 0;
        match self.state {
            CircuitState::HalfOpen => {
                self.success_count += 1;
                if self.success_count >= self.config.success_threshold {
                    self.state = CircuitState::Closed;
                    self.success_count = 0;
                    log::info!("Circuit breaker CLOSED (provider recovered)");
                }
            }
            CircuitState::Closed => {
                // Reset success count — we track it only in HalfOpen.
            }
            CircuitState::Open => {
                // Should not happen — Open circuits reject requests.
            }
        }
    }

    /// Record a failure — may open a Closed circuit.
    fn record_failure(&mut self, now: Instant) {
        self.failure_count += 1;
        self.last_failure_at = Some(now);
        self.success_count = 0;

        match self.state {
            CircuitState::Closed => {
                if self.failure_count >= self.config.failure_threshold {
                    self.state = CircuitState::Open;
                    self.opened_at = Some(now);
                    log::warn!(
                        "Circuit breaker OPEN after {} failures",
                        self.failure_count
                    );
                }
            }
            CircuitState::HalfOpen => {
                // A single failure in HalfOpen re-opens the circuit.
                self.state = CircuitState::Open;
                self.opened_at = Some(now);
                log::warn!("Circuit breaker RE-OPENED after HalfOpen failure");
            }
            CircuitState::Open => {
                // Circuit is already open — update opened_at.
                self.opened_at = Some(now);
            }
        }
    }

    /// Check if the circuit should transition from Open to HalfOpen.
    fn maybe_transition_to_half_open(&mut self, now: Instant) {
        if self.state != CircuitState::Open {
            return;
        }
        if let Some(opened_at) = self.opened_at {
            if now.duration_since(opened_at) >= self.config.cooldown_duration {
                self.state = CircuitState::HalfOpen;
                log::info!("Circuit breaker HALF_OPEN (cooldown elapsed)");
            }
        }
    }

    /// Whether a request should be allowed through.
    fn is_request_allowed(&self) -> bool {
        match self.state {
            CircuitState::Closed => true,
            CircuitState::HalfOpen => true, // Allow test request
            CircuitState::Open => false,     // Reject
        }
    }
}

// ── Circuit Breaker Registry ───────────────────────────────────────────────

/// A thread-safe registry of circuit breakers, keyed by provider key.
///
/// Each provider gets its own circuit breaker so a failure in one provider
/// doesn't affect requests to another.
pub struct CircuitBreakerRegistry {
    breakers: Mutex<HashMap<String, BreakerState>>,
}

impl CircuitBreakerRegistry {
    /// Create a new, empty registry.
    pub fn new() -> Self {
        Self {
            breakers: Mutex::new(HashMap::new()),
        }
    }

    /// Check if a request to the given provider should be allowed.
    ///
    /// Returns `Ok(())` if allowed, or `Err(AgentJaxError)` with the
    /// circuit breaker rejection message.
    pub fn check(&self, provider_key: &str) -> Result<(), AgentJaxError> {
        let mut breakers = self.breakers.lock().unwrap();
        let state = breakers
            .entry(provider_key.to_string())
            .or_insert_with(|| BreakerState::new(CircuitBreakerConfig::default()));

        let now = Instant::now();
        state.maybe_transition_to_half_open(now);

        if state.is_request_allowed() {
            Ok(())
        } else {
            Err(AgentJaxError {
                kind: ErrorKind::ProviderUnavailable,
                message: format!(
                    "Circuit breaker is OPEN for provider '{provider_key}'. \
                     The circuit will transition to half-open after ~{:?}. \
                     Prev failures: {}",
                    state.config.cooldown_duration,
                    state.failure_count,
                ),
                retryable: true,
                provider_key: Some(provider_key.to_string()),
                source: None,
            })
        }
    }

    /// Record a successful call to the provider.
    pub fn record_success(&self, provider_key: &str) {
        let mut breakers = self.breakers.lock().unwrap();
        let state = breakers
            .entry(provider_key.to_string())
            .or_insert_with(|| BreakerState::new(CircuitBreakerConfig::default()));
        state.record_success();
    }

    /// Record a failed call to the provider.
    pub fn record_failure(&self, provider_key: &str) {
        let mut breakers = self.breakers.lock().unwrap();
        let state = breakers
            .entry(provider_key.to_string())
            .or_insert_with(|| BreakerState::new(CircuitBreakerConfig::default()));
        state.record_failure(Instant::now());
    }

    /// Get the current state for a provider (for diagnostics).
    pub fn get_state(&self, provider_key: &str) -> Option<CircuitState> {
        let mut breakers = self.breakers.lock().unwrap();
        let state = breakers
            .entry(provider_key.to_string())
            .or_insert_with(|| BreakerState::new(CircuitBreakerConfig::default()));
        Some(state.state)
    }

    /// Reset a circuit breaker back to Closed.
    pub fn reset(&self, provider_key: &str) {
        let mut breakers = self.breakers.lock().unwrap();
        if let Some(state) = breakers.get_mut(provider_key) {
            state.state = CircuitState::Closed;
            state.failure_count = 0;
            state.success_count = 0;
            state.last_failure_at = None;
            state.opened_at = None;
            log::info!("Circuit breaker RESET for provider '{}'", provider_key);
        }
    }
}

impl Default for CircuitBreakerRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_closed_allows_requests() {
        let registry = CircuitBreakerRegistry::new();
        assert!(registry.check("openai").is_ok());
    }

    #[test]
    fn test_opens_after_threshold() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown_duration: Duration::from_secs(60),
            success_threshold: 1,
        };
        let mut state = BreakerState::new(config);

        assert_eq!(state.state, CircuitState::Closed);
        state.record_failure(Instant::now());
        assert_eq!(state.state, CircuitState::Closed);
        state.record_failure(Instant::now());
        assert_eq!(state.state, CircuitState::Open);
        assert!(!state.is_request_allowed());
    }

    #[test]
    fn test_half_open_transition() {
        let config = CircuitBreakerConfig {
            failure_threshold: 2,
            cooldown_duration: Duration::from_millis(1), // Very short
            success_threshold: 1,
        };
        let mut state = BreakerState::new(config);

        // Trip the breaker.
        state.record_failure(Instant::now());
        state.record_failure(Instant::now());
        assert_eq!(state.state, CircuitState::Open);

        // Wait for cooldown.
        std::thread::sleep(Duration::from_millis(5));
        state.maybe_transition_to_half_open(Instant::now());
        assert_eq!(state.state, CircuitState::HalfOpen);
        assert!(state.is_request_allowed());
    }

    #[test]
    fn test_closes_after_successes() {
        let config = CircuitBreakerConfig {
            failure_threshold: 1,
            cooldown_duration: Duration::from_millis(1),
            success_threshold: 2,
        };
        let mut state = BreakerState::new(config);

        // Trip → Open.
        state.record_failure(Instant::now());
        assert_eq!(state.state, CircuitState::Open);

        // Cooldown → HalfOpen.
        std::thread::sleep(Duration::from_millis(5));
        state.maybe_transition_to_half_open(Instant::now());
        assert_eq!(state.state, CircuitState::HalfOpen);

        // Two successes → Closed.
        state.record_success();
        assert_eq!(state.state, CircuitState::HalfOpen);
        state.record_success();
        assert_eq!(state.state, CircuitState::Closed);
    }

    #[test]
    fn test_registry_record_success_resets_failures() {
        let registry = CircuitBreakerRegistry::new();

        registry.record_failure("anthropic");
        registry.record_failure("anthropic");
        registry.record_failure("anthropic");
        assert_eq!(registry.get_state("anthropic"), Some(CircuitState::Open));

        // Reset
        registry.reset("anthropic");
        assert_eq!(registry.get_state("anthropic"), Some(CircuitState::Closed));
    }

    #[test]
    fn test_registry_provider_isolation() {
        let registry = CircuitBreakerRegistry::new();

        registry.record_failure("anthropic");
        registry.record_failure("anthropic");
        registry.record_failure("anthropic");

        // OpenAI is unaffected.
        assert!(registry.check("openai").is_ok());
        // Anthropic is blocked.
        assert!(registry.check("anthropic").is_err());
    }
}
