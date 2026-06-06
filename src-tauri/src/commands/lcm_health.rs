//! Tauri commands for LCM health dashboard.
//!
//! Exposes integrity checks, circuit breaker state, spend guard state,
//! and session metrics to the frontend UI.

use crate::lcm::LcmConfig;
use crate::lcm::circuit_breaker::CircuitBreaker;
use crate::lcm::integrity::{IntegrityChecker, IntegrityReport, LcmMetrics};
use crate::lcm::spend_guard::SpendGuard;
use serde::Serialize;
use std::sync::{Arc, OnceLock};

// ── Singleton macro ─────────────────────────────────────────────────────────

/// Define a function returning a lazily-initialized `&'static T` via `OnceLock`.
macro_rules! lazy_singleton {
    ($name:ident, $ty:ty) => {
        fn $name() -> &'static $ty {
            static INSTANCE: OnceLock<$ty> = OnceLock::new();
            INSTANCE.get_or_init(|| <$ty>::default())
        }
    };
}

// ── Shared State ────────────────────────────────────────────────────────────

lazy_singleton!(global_circuit_breaker, CircuitBreaker);
lazy_singleton!(global_spend_guard, SpendGuard);

// ── Response Types ──────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LcmHealthResponse {
    pub integrity: Option<IntegrityReport>,
    pub metrics: Option<LcmMetrics>,
    pub circuit_breaker: Vec<crate::lcm::circuit_breaker::BreakerEntry>,
    pub spend_guard: Vec<crate::lcm::spend_guard::SpendGuardEntry>,
    pub config: LcmHealthConfig,
    pub repair_suggestions: Vec<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LcmHealthConfig {
    pub soft_token_threshold: u32,
    pub hard_token_threshold: u32,
    pub compaction_timeout_secs: u32,
    pub max_compact_block_size: usize,
    pub truncation_max_tokens: u32,
    pub summarization_model: String,
    pub dynamic_thresholds: bool,
}

impl From<&LcmConfig> for LcmHealthConfig {
    fn from(c: &LcmConfig) -> Self {
        Self {
            soft_token_threshold: c.soft_token_threshold,
            hard_token_threshold: c.hard_token_threshold,
            compaction_timeout_secs: c.compaction_timeout_secs,
            max_compact_block_size: c.max_compact_block_size,
            truncation_max_tokens: c.truncation_max_tokens,
            summarization_model: c.summarization_model.clone(),
            dynamic_thresholds: c.dynamic_thresholds,
        }
    }
}

// ── Commands ────────────────────────────────────────────────────────────────

/// Get the full LCM health dashboard data.
#[tauri::command]
pub fn get_lcm_health(
    agent_id: String,
    conversation_id: String,
) -> Result<LcmHealthResponse, crate::error::AgentJaxError> {
    let agent_config = crate::config::load_agent_config(&agent_id)
        .unwrap_or_default()
        .normalize();

    let lcm_config_types = agent_config
        .context_management
        .to_lcm_config()
        .with_dynamic_thresholds(128_000);

    // IMPORTANT: Only open LCM store if it already exists.
    // LcmStore::open() creates the DB and directory if missing, which would
    // leave empty files for conversations that only exist as frontend UUIDs.
    let db_path = match crate::lcm::lcm_store_path(&agent_id, &conversation_id) {
        Ok(p) => p,
        Err(e) => {
            return Ok(LcmHealthResponse {
                integrity: None,
                metrics: None,
                circuit_breaker: global_circuit_breaker().snapshot(),
                spend_guard: global_spend_guard().snapshot(),
                config: LcmHealthConfig::from(&lcm_config_types),
                repair_suggestions: vec![format!("Cannot resolve LCM path: {e}")],
            });
        }
    };

    let (integrity_report, metrics, repair_suggestions) = if db_path.exists() {
        match crate::lcm::store::LcmStore::open(&db_path, lcm_config_types.clone()) {
            Ok(store) => {
                let store = Arc::new(store);
                let checker = IntegrityChecker::new(store.clone());

                let report = checker.scan(&conversation_id).ok();
                let met = checker.collect_metrics(&conversation_id).ok();
                let suggestions = report
                    .as_ref()
                    .map(|r| crate::lcm::integrity::repair_plan(r))
                    .unwrap_or_default();

                (report, met, suggestions)
            }
            Err(e) => (None, None, vec![format!("Failed to open LCM store: {e}")]),
        }
    } else {
        (None, None, Vec::new())
    };

    Ok(LcmHealthResponse {
        integrity: integrity_report,
        metrics,
        circuit_breaker: global_circuit_breaker().snapshot(),
        spend_guard: global_spend_guard().snapshot(),
        config: LcmHealthConfig::from(&lcm_config_types),
        repair_suggestions,
    })
}

/// Reset the circuit breaker for a specific key (or all if key is empty).
#[tauri::command]
pub fn reset_circuit_breaker(key: Option<String>) -> Result<(), crate::error::AgentJaxError> {
    match key {
        Some(k) if !k.is_empty() => {
            global_circuit_breaker().reset(&k);
        }
        _ => {
            global_circuit_breaker().reset_all();
        }
    }
    Ok(())
}

/// Reset the spend guard for a specific key (or all if key is empty).
#[tauri::command]
pub fn reset_spend_guard(key: Option<String>) -> Result<(), crate::error::AgentJaxError> {
    match key {
        Some(k) if !k.is_empty() => {
            global_spend_guard().reset(&k);
        }
        _ => {
            global_spend_guard().reset_all();
        }
    }
    Ok(())
}

/// Record a summarization failure (opens circuit breaker if threshold exceeded).
///
/// This is called by the summarizer when a provider returns an auth error.
#[tauri::command]
pub fn record_summarization_failure(
    provider: String,
    model: String,
    reason: String,
) -> Result<(), crate::error::AgentJaxError> {
    let key = CircuitBreaker::build_key(&provider, &model);
    global_circuit_breaker().record_failure(&key, &reason);
    Ok(())
}

/// Record a summarization success (resets circuit breaker counter).
#[tauri::command]
pub fn record_summarization_success(provider: String, model: String) -> Result<(), crate::error::AgentJaxError> {
    let key = CircuitBreaker::build_key(&provider, &model);
    global_circuit_breaker().record_success(&key);
    Ok(())
}

/// Check if the circuit breaker is open for a given provider/model.
#[tauri::command]
pub fn is_circuit_breaker_open(provider: String, model: String) -> Result<bool, crate::error::AgentJaxError> {
    let key = CircuitBreaker::build_key(&provider, &model);
    Ok(global_circuit_breaker().is_open(&key))
}

/// Check if summarization calls are allowed for a given model (spend guard).
#[tauri::command]
pub fn is_summarization_allowed(key: String) -> Result<bool, crate::error::AgentJaxError> {
    Ok(global_spend_guard().is_allowed(&key))
}
