//! Tauri commands for LCM health dashboard.
//!
//! Exposes integrity checks, circuit breaker state, spend guard state,
//! session metrics, and repair execution to the frontend UI.

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
    /// Metrics sourced from JSONL (the ground truth), for comparison.
    pub jsonl_metrics: Option<JsonLMetrics>,
    /// Whether a JSONL→LCM backfill can be performed (JSONL exists, LCM is empty/stale).
    pub backfill_available: bool,
    pub circuit_breaker: Vec<crate::lcm::circuit_breaker::BreakerEntry>,
    pub spend_guard: Vec<crate::lcm::spend_guard::SpendGuardEntry>,
    pub config: LcmHealthConfig,
    pub repair_suggestions: Vec<String>,
}

/// Lightweight metrics computed from the JSONL session file directly.
/// These are always accurate because JSONL is the append-only source of truth.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JsonLMetrics {
    pub message_count: usize,
    pub user_message_count: usize,
    pub assistant_message_count: usize,
    pub tool_message_count: usize,
    pub estimated_total_tokens: u32,
    pub file_size_bytes: u64,
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
    /// The context window used to compute dynamic thresholds (0 if not resolved).
    pub resolved_context_window: u32,
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
            resolved_context_window: 0, // filled in by caller below
        }
    }
}

// ── Commands ────────────────────────────────────────────────────────────────

/// Read JSONL-sourced metrics directly from the messages.jsonl file.
/// This is the ground truth — always accurate regardless of LCM state.
fn collect_jsonl_metrics(
    agent_id: &str,
    conversation_id: &str,
) -> Option<JsonLMetrics> {
    let messages_path = crate::conversation_store::paths::conversation_messages_path(agent_id, conversation_id).ok()?;
    if !messages_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&messages_path).ok()?;
    let file_size_bytes = content.len() as u64;
    let mut message_count: usize = 0;
    let mut user_count: usize = 0;
    let mut assistant_count: usize = 0;
    let mut tool_count: usize = 0;
    let mut total_tokens: u32 = 0;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Quick tag-based parsing without full deserialization
        let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(line) else {
            continue;
        };
        message_count += 1;

        // Count by role
        if let Some(tag) = v.get("tag").and_then(|t| t.as_str()) {
            match tag {
                "user" => {
                    user_count += 1;
                    if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                        total_tokens += crate::lcm::types::estimate_tokens(text);
                    }
                }
                "assistant" => {
                    assistant_count += 1;
                    if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
                        total_tokens += crate::lcm::types::estimate_tokens(text);
                    }
                }
                "tool" => {
                    tool_count += 1;
                    // Count both args and output
                    if let Some(args) = v.get("args") {
                        total_tokens += crate::lcm::types::estimate_tokens(&args.to_string());
                    }
                    if let Some(output) = v.get("output").and_then(|o| o.as_str()) {
                        total_tokens += crate::lcm::types::estimate_tokens(output);
                    }
                }
                _ => {}
            }
        }
    }

    Some(JsonLMetrics {
        message_count,
        user_message_count: user_count,
        assistant_message_count: assistant_count,
        tool_message_count: tool_count,
        estimated_total_tokens: total_tokens,
        file_size_bytes,
    })
}

/// Check whether a JSONL→LCM backfill would be useful.
fn check_backfill_available(
    agent_id: &str,
    conversation_id: &str,
    db_path: &std::path::Path,
) -> bool {
    // Does JSONL exist?
    let messages_path = match crate::conversation_store::paths::conversation_messages_path(agent_id, conversation_id) {
        Ok(p) => p,
        Err(_) => return false,
    };
    if !messages_path.exists() {
        return false;
    }

    // Is LCM empty/stale?
    if !db_path.exists() {
        return true; // LCM doesn't exist at all — backfill would create it
    }

    // LCM exists — check if it has less data than JSONL
    match crate::lcm::store::LcmStore::open(db_path, crate::lcm::LcmConfig::default()) {
        Ok(store) => {
            let lcm_count = store.get_conversation_messages(conversation_id)
                .map(|m| m.len())
                .unwrap_or(0);
            // Count JSONL lines quickly
            let jsonl_count = std::fs::read_to_string(&messages_path)
                .map(|c| c.lines().filter(|l| !l.trim().is_empty()).count())
                .unwrap_or(0);
            lcm_count == 0 || jsonl_count > lcm_count
        }
        Err(_) => false,
    }
}

/// Resolve the actual context window for the active provider/model.
fn resolve_context_window(app_config: &crate::config::AppConfig) -> u32 {
    // Try to find any provider with models to get a context window estimate
    for provider in app_config.providers.values() {
        for model in provider.models.values() {
            if let Some(window) = model.request.max_output_tokens.map(|t| t * 4) {
                return window;
            }
        }
    }
    // Fallback: typical large context window
    128_000
}

/// Get the full LCM health dashboard data.
#[tauri::command]
pub fn get_lcm_health(
    agent_id: String,
    conversation_id: String,
) -> Result<LcmHealthResponse, crate::error::AgentJaxError> {
    let app_config = crate::config::load_config().unwrap_or_default();
    let agent_config = crate::config::load_agent_config(&agent_id)
        .unwrap_or_default()
        .normalize();

    let context_window = resolve_context_window(&app_config);
    let lcm_config_types = agent_config
        .context_management
        .to_lcm_config()
        .with_dynamic_thresholds(context_window as usize);

    // IMPORTANT: Only open LCM store if it already exists.
    // LcmStore::open() creates the DB and directory if missing, which would
    // leave empty files for conversations that only exist as frontend UUIDs.
    let db_path = match crate::lcm::lcm_store_path(&agent_id, &conversation_id) {
        Ok(p) => p,
        Err(e) => {
            let jsonl_metrics = collect_jsonl_metrics(&agent_id, &conversation_id);
            let mut config = LcmHealthConfig::from(&lcm_config_types);
            config.resolved_context_window = context_window;
            return Ok(LcmHealthResponse {
                integrity: None,
                metrics: None,
                jsonl_metrics,
                backfill_available: false,
                circuit_breaker: global_circuit_breaker().snapshot(),
                spend_guard: global_spend_guard().snapshot(),
                config,
                repair_suggestions: vec![format!("Cannot resolve LCM path: {e}")],
            });
        }
    };

    let jsonl_metrics = collect_jsonl_metrics(&agent_id, &conversation_id);
    let backfill_available = check_backfill_available(&agent_id, &conversation_id, &db_path);

    let (integrity_report, metrics, repair_suggestions) = if db_path.exists() {
        match crate::lcm::store::LcmStore::open(&db_path, lcm_config_types.clone()) {
            Ok(store) => {
                let store = Arc::new(store);
                let checker = IntegrityChecker::new(store.clone());

                let report = checker.scan(&conversation_id).ok();
                let met = checker.collect_metrics(&conversation_id).ok();
                let suggestions = report
                    .as_ref()
                    .map(|r| crate::lcm::integrity::repair_plan_with_actions(r, backfill_available))
                    .unwrap_or_default();

                (report, met, suggestions)
            }
            Err(e) => (None, None, vec![format!("Failed to open LCM store: {e}")]),
        }
    } else {
        let mut suggestions = Vec::new();
        if backfill_available {
            suggestions.push(
                "LCM store is missing but JSONL data exists. Click 'Backfill from JSONL' to populate the LCM store.".to_string()
            );
        }
        (None, None, suggestions)
    };

    let mut config = LcmHealthConfig::from(&lcm_config_types);
    config.resolved_context_window = context_window;

    Ok(LcmHealthResponse {
        integrity: integrity_report,
        metrics,
        jsonl_metrics,
        backfill_available,
        circuit_breaker: global_circuit_breaker().snapshot(),
        spend_guard: global_spend_guard().snapshot(),
        config,
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

// ── Repair & Backfill Execution ────────────────────────────────────────────

/// Trigger JSONL→LCM backfill for a conversation.
///
/// Reads messages.jsonl and populates the LCM store. This is safe to call
/// multiple times — it skips backfill if LCM already has data.
#[tauri::command]
pub fn trigger_lcm_backfill(
    agent_id: String,
    conversation_id: String,
) -> Result<LcmRepairResult, crate::error::AgentJaxError> {
    match crate::lcm::backfill_lcm_from_jsonl(&agent_id, &conversation_id) {
        Ok(true) => Ok(LcmRepairResult {
            success: true,
            message: format!("Successfully backfilled conversation '{}' from JSONL to LCM store.", conversation_id),
        }),
        Ok(false) => Ok(LcmRepairResult {
            success: true,
            message: format!("Backfill not needed: conversation '{}' is already up-to-date in LCM store.", conversation_id),
        }),
        Err(e) => Ok(LcmRepairResult {
            success: false,
            message: format!("Backfill failed: {e}"),
        }),
    }
}

/// Execute repair actions for a conversation's LCM store.
///
/// Currently supports:
/// - "remove_orphan_summaries": Deletes summary nodes with no lineage
/// - "recompute_message_sequence": Fixes message seq ordering
/// - "ensure_conversation_meta": Creates conversation metadata if missing
#[tauri::command]
pub fn execute_lcm_repair(
    agent_id: String,
    conversation_id: String,
    action: String,
) -> Result<LcmRepairResult, crate::error::AgentJaxError> {
    let db_path = match crate::lcm::lcm_store_path(&agent_id, &conversation_id) {
        Ok(p) => p,
        Err(e) => {
            return Ok(LcmRepairResult {
                success: false,
                message: format!("Cannot resolve LCM path: {e}"),
            });
        }
    };

    if !db_path.exists() {
        return Ok(LcmRepairResult {
            success: false,
            message: "LCM store does not exist. Try 'Backfill from JSONL' first.".to_string(),
        });
    }

    let lcm_config = crate::lcm::LcmConfig::default();
    let store = match crate::lcm::store::LcmStore::open(&db_path, lcm_config) {
        Ok(s) => s,
        Err(e) => {
            return Ok(LcmRepairResult {
                success: false,
                message: format!("Failed to open LCM store: {e}"),
            });
        }
    };

    match action.as_str() {
        "remove_orphan_summaries" => {
            match store.delete_orphan_summaries(&conversation_id) {
                Ok(count) => Ok(LcmRepairResult {
                    success: true,
                    message: format!("Removed {count} orphan summary records."),
                }),
                Err(e) => Ok(LcmRepairResult {
                    success: false,
                    message: format!("Failed to remove orphan summaries: {e}"),
                }),
            }
        }
        "recompute_message_sequence" => {
            match store.repair_message_sequence(&conversation_id) {
                Ok(count) => Ok(LcmRepairResult {
                    success: true,
                    message: format!("Re-indexed sequence for {count} messages."),
                }),
                Err(e) => Ok(LcmRepairResult {
                    success: false,
                    message: format!("Failed to re-index message sequence: {e}"),
                }),
            }
        }
        "ensure_conversation_meta" => {
            match store.ensure_conversation_meta(&conversation_id) {
                Ok(meta) => Ok(LcmRepairResult {
                    success: true,
                    message: format!("Conversation metadata ensured (title: '{}', version: {}).", meta.title, meta.version),
                }),
                Err(e) => Ok(LcmRepairResult {
                    success: false,
                    message: format!("Failed to ensure conversation metadata: {e}"),
                }),
            }
        }
        "repair_lineage" => {
            match store.repair_summary_lineage(&conversation_id) {
                Ok(count) => Ok(LcmRepairResult {
                    success: true,
                    message: format!("Repaired lineage for {count} summary nodes."),
                }),
                Err(e) => Ok(LcmRepairResult {
                    success: false,
                    message: format!("Failed to repair summary lineage: {e}"),
                }),
            }
        }
        other => Ok(LcmRepairResult {
            success: false,
            message: format!("Unknown repair action: '{other}'. Available: remove_orphan_summaries, recompute_message_sequence, ensure_conversation_meta, repair_lineage"),
        }),
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LcmRepairResult {
    pub success: bool,
    pub message: String,
}
