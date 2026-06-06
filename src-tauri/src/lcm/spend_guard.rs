//! Spending protection for summarization calls.
//!
//! Prevents excessive summarization calls from consuming too many tokens
//! within a rolling time window. After `max_calls` in `window_duration`,
//! further calls are blocked until the window rolls over or a backoff
//! period elapses.
//!
//! Inspired by lossless-claw's `SummarySpendGuardState` in `summarize.ts`.

use serde::Serialize;
use std::collections::VecDeque;
use std::sync::Mutex;
use std::time::{Duration, Instant};

/// State of the spend guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SpendGuardState {
    /// Normal operation — calls allowed.
    Normal,
    /// Backing off — calls blocked until backoff ends.
    BackingOff,
}

/// A single entry in the spend guard snapshot.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SpendGuardEntry {
    pub key: String,
    pub state: SpendGuardState,
    pub calls_in_window: usize,
    pub max_calls: usize,
    pub window_duration_secs: u64,
    pub backoff_remaining_secs: Option<u64>,
}

/// Configuration for a spend guard.
#[derive(Debug, Clone)]
pub struct SpendGuardConfig {
    /// Rolling window duration.
    pub window: Duration,
    /// Maximum calls allowed within the window.
    pub max_calls: usize,
    /// Backoff duration after hitting the limit.
    pub backoff: Duration,
}

impl Default for SpendGuardConfig {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(600), // 10 minutes
            max_calls: 24,
            backoff: Duration::from_secs(1800), // 30 minutes
        }
    }
}

/// Spending protection guard.
pub struct SpendGuard {
    config: SpendGuardConfig,
    /// Per-key call timestamps (FIFO queue).
    calls: Mutex<VecDeque<(String, Instant)>>,
    /// Per-key backoff end times.
    backoffs: Mutex<Vec<(String, Instant)>>,
}

impl SpendGuard {
    /// Create a new spend guard.
    pub fn new(config: SpendGuardConfig) -> Self {
        Self {
            config,
            calls: Mutex::new(VecDeque::new()),
            backoffs: Mutex::new(Vec::new()),
        }
    }

    /// Create with default config.
    pub fn default() -> Self {
        Self::new(SpendGuardConfig::default())
    }

    /// Check if a call is allowed for the given key.
    ///
    /// Returns `true` if the call should proceed.
    pub fn is_allowed(&self, key: &str) -> bool {
        let mut backoffs = self.backoffs.lock().unwrap();
        let now = Instant::now();

        // Check backoff.
        backoffs.retain(|(k, until)| {
            if k != key {
                return true;
            }
            now < *until
        });

        let in_backoff = backoffs.iter().any(|(k, until)| k == key && now < *until);
        if in_backoff {
            return false;
        }

        // Count calls within the window.
        let mut calls = self.calls.lock().unwrap();
        let window_start = now - self.config.window;

        // Remove old entries.
        while calls
            .front()
            .map(|(_, t)| *t < window_start)
            .unwrap_or(false)
        {
            calls.pop_front();
        }

        // Count calls for this key.
        let key_calls = calls.iter().filter(|(k, _)| k == key).count();
        if key_calls >= self.config.max_calls {
            // Enter backoff.
            let backoff_until = now + self.config.backoff;
            if !backoffs.iter().any(|(k, _)| k == key) {
                backoffs.push((key.to_string(), backoff_until));
            }
            return false;
        }

        true
    }

    /// Get the current state snapshot.
    pub fn snapshot(&self) -> Vec<SpendGuardEntry> {
        let calls = self.calls.lock().unwrap();
        let backoffs = self.backoffs.lock().unwrap();
        let now = Instant::now();
        let window_start = now - self.config.window;

        // Collect unique keys.
        let mut keys: Vec<String> = calls.iter().map(|(k, _)| k.clone()).collect();
        keys.extend(backoffs.iter().map(|(k, _)| k.clone()));
        keys.sort();
        keys.dedup();

        keys.into_iter()
            .map(|key| {
                let key_calls = calls
                    .iter()
                    .filter(|(k, t)| k == &key && *t >= window_start)
                    .count();
                let in_backoff = backoffs.iter().any(|(k, until)| k == &key && now < *until);
                let backoff_remaining = backoffs
                    .iter()
                    .find(|(k, _)| k == &key)
                    .map(|(_, until)| until.saturating_duration_since(now).as_secs());

                SpendGuardEntry {
                    key,
                    state: if in_backoff {
                        SpendGuardState::BackingOff
                    } else {
                        SpendGuardState::Normal
                    },
                    calls_in_window: key_calls,
                    max_calls: self.config.max_calls,
                    window_duration_secs: self.config.window.as_secs(),
                    backoff_remaining_secs: if in_backoff { backoff_remaining } else { None },
                }
            })
            .collect()
    }

    /// Reset all state for a key.
    pub fn reset(&self, key: &str) {
        let mut calls = self.calls.lock().unwrap();
        calls.retain(|(k, _)| k != key);
        let mut backoffs = self.backoffs.lock().unwrap();
        backoffs.retain(|(k, _)| k != key);
    }

    /// Reset all state.
    pub fn reset_all(&self) {
        let mut calls = self.calls.lock().unwrap();
        calls.clear();
        let mut backoffs = self.backoffs.lock().unwrap();
        backoffs.clear();
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_reset_clears() {
        let guard = SpendGuard::new(SpendGuardConfig {
            window: Duration::from_secs(60),
            max_calls: 1,
            backoff: Duration::from_secs(60),
        });

        assert!(guard.is_allowed("test::key"));
        guard.reset("test::key");
        assert!(guard.is_allowed("test::key"));
    }

    #[test]
    fn test_snapshot() {
        let guard = SpendGuard::default();
        let snap = guard.snapshot();
        assert!(snap.is_empty());
    }
}
