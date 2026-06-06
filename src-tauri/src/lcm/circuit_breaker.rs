//! Circuit breaker for summarization provider calls.
//!
//! Prevents repeated summarization attempts against a provider/model that
//! is failing with authentication or server errors. After `threshold`
//! consecutive failures, the breaker opens for `cooldown` duration.
//!
//! Inspired by lossless-claw's circuit breaker in `summarize.ts`.

use serde::Serialize;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// State of the circuit breaker for a single provider/model key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum BreakerState {
    Closed,
    Open,
}

/// A single breaker entry.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BreakerEntry {
    pub key: String,
    pub state: BreakerState,
    pub consecutive_failures: u32,
    pub blocked_until: Option<i64>, // unix ms
    pub last_failure_reason: Option<String>,
}

/// Circuit breaker configuration.
#[derive(Debug, Clone)]
pub struct CircuitBreakerConfig {
    /// Number of consecutive failures before opening.
    pub threshold: u32,
    /// Duration to stay open before resetting to half-open.
    pub cooldown: Duration,
}

impl Default for CircuitBreakerConfig {
    fn default() -> Self {
        Self {
            threshold: 5,
            cooldown: Duration::from_secs(1800), // 30 minutes
        }
    }
}

/// Per-provider/model circuit breaker.
pub struct CircuitBreaker {
    config: CircuitBreakerConfig,
    failures: Mutex<HashMap<String, (u32, Instant)>>,
    blocked: Mutex<HashMap<String, Instant>>,
}

impl CircuitBreaker {
    /// Create a new circuit breaker with the given config.
    pub fn new(config: CircuitBreakerConfig) -> Self {
        Self {
            config,
            failures: Mutex::new(HashMap::new()),
            blocked: Mutex::new(HashMap::new()),
        }
    }

    /// Create with default config (threshold=5, cooldown=30min).
    pub fn default() -> Self {
        Self::new(CircuitBreakerConfig::default())
    }

    /// Build a breaker key from provider and model identifiers.
    pub fn build_key(provider: &str, model: &str) -> String {
        format!("{provider}::{model}")
    }

    /// Check if the breaker is open for the given key.
    ///
    /// Returns `true` if calls should be blocked.
    pub fn is_open(&self, key: &str) -> bool {
        let blocked = self.blocked.lock().unwrap();
        if let Some(until) = blocked.get(key) {
            if Instant::now() < *until {
                return true;
            }
            // Cooldown expired — remove entry (next call resets).
            drop(blocked);
            let mut blocked = self.blocked.lock().unwrap();
            blocked.remove(key);
            let mut failures = self.failures.lock().unwrap();
            failures.remove(key);
            return false;
        }
        false
    }

    /// Record a successful call — resets failure count.
    pub fn record_success(&self, key: &str) {
        let mut failures = self.failures.lock().unwrap();
        failures.remove(key);
    }

    /// Record a failure — may open the breaker.
    pub fn record_failure(&self, key: &str, _reason: &str) {
        let mut failures = self.failures.lock().unwrap();
        let entry = failures
            .entry(key.to_string())
            .or_insert((0, Instant::now()));
        entry.0 += 1;
        entry.1 = Instant::now();

        if entry.0 >= self.config.threshold {
            // Open the breaker.
            let mut blocked = self.blocked.lock().unwrap();
            blocked.insert(key.to_string(), Instant::now() + self.config.cooldown);
        }
    }

    /// Get the current state for all tracked keys.
    pub fn snapshot(&self) -> Vec<BreakerEntry> {
        let failures = self.failures.lock().unwrap();
        let blocked = self.blocked.lock().unwrap();
        let now = Instant::now();

        let mut entries: Vec<BreakerEntry> = failures
            .iter()
            .map(|(key, (count, _))| {
                let blocked_until = blocked.get(key).map(|until| {
                    let remaining = until.saturating_duration_since(now);
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_millis() as i64
                        + remaining.as_millis() as i64
                });

                BreakerEntry {
                    key: key.clone(),
                    state: if blocked.contains_key(key) {
                        BreakerState::Open
                    } else {
                        BreakerState::Closed
                    },
                    consecutive_failures: *count,
                    blocked_until,
                    last_failure_reason: None,
                }
            })
            .collect();

        // Also include keys that are blocked but have no failure entries
        // (can happen after cleanup).
        for (key, until) in blocked.iter() {
            if !entries.iter().any(|e| e.key == *key) {
                let remaining = until.saturating_duration_since(now);
                let blocked_until = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap_or_default()
                    .as_millis() as i64
                    + remaining.as_millis() as i64;

                entries.push(BreakerEntry {
                    key: key.clone(),
                    state: BreakerState::Open,
                    consecutive_failures: 0,
                    blocked_until: Some(blocked_until),
                    last_failure_reason: None,
                });
            }
        }

        entries.sort_by(|a, b| a.key.cmp(&b.key));
        entries
    }

    /// Reset the breaker for a specific key.
    pub fn reset(&self, key: &str) {
        let mut failures = self.failures.lock().unwrap();
        let mut blocked = self.blocked.lock().unwrap();
        failures.remove(key);
        blocked.remove(key);
    }

    /// Reset all breakers.
    pub fn reset_all(&self) {
        let mut failures = self.failures.lock().unwrap();
        let mut blocked = self.blocked.lock().unwrap();
        failures.clear();
        blocked.clear();
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_breaker_starts_closed() {
        let breaker = CircuitBreaker::default();
        assert!(!breaker.is_open("test::model"));
    }

    #[test]
    fn test_breaker_opens_after_threshold() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 3,
            cooldown: Duration::from_secs(60),
        });

        assert!(!breaker.is_open("test::model"));
        breaker.record_failure("test::model", "auth error");
        assert!(!breaker.is_open("test::model"));
        breaker.record_failure("test::model", "auth error");
        assert!(!breaker.is_open("test::model"));
        breaker.record_failure("test::model", "auth error");
        assert!(breaker.is_open("test::model"));
    }

    #[test]
    fn test_success_resets_counter() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 3,
            cooldown: Duration::from_secs(60),
        });

        breaker.record_failure("test::model", "error");
        breaker.record_failure("test::model", "error");
        breaker.record_success("test::model");
        // Should still be closed after success
        assert!(!breaker.is_open("test::model"));
        // Two more failures should not open (counter reset)
        breaker.record_failure("test::model", "error");
        breaker.record_failure("test::model", "error");
        assert!(!breaker.is_open("test::model"));
        // Third failure after reset should open
        breaker.record_failure("test::model", "error");
        assert!(breaker.is_open("test::model"));
    }

    #[test]
    fn test_snapshot() {
        let breaker = CircuitBreaker::default();
        breaker.record_failure("p1::m1", "error");
        breaker.record_failure("p1::m1", "error");
        let snap = breaker.snapshot();
        assert!(!snap.is_empty());
        let entry = snap.iter().find(|e| e.key == "p1::m1").unwrap();
        assert_eq!(entry.consecutive_failures, 2);
        assert_eq!(entry.state, BreakerState::Closed);
    }

    #[test]
    fn test_reset() {
        let breaker = CircuitBreaker::new(CircuitBreakerConfig {
            threshold: 1,
            cooldown: Duration::from_secs(60),
        });
        breaker.record_failure("test::model", "error");
        assert!(breaker.is_open("test::model"));
        breaker.reset("test::model");
        assert!(!breaker.is_open("test::model"));
    }
}
