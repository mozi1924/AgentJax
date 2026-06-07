//! LCM (Lossless Context Management) module.
//!
//! Implements the deterministic, engine-driven context management architecture
//! described in "LCM: Lossless Context Management" (Ehrlich & Blackman, 2026).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────┐
//! │                  LcmEngine                       │
//! │  ┌──────────────┐  ┌────────────┐  ┌──────────┐ │
//! │  │ Immutable    │  │ Summary    │  │ Active    │ │
//! │  │ Store (SQLite)│  │ DAG        │  │ Context   │ │
//! │  │              │  │            │  │ Assembler │ │
//! │  │ messages     │  │ summaries  │  │           │ │
//! │  │ file_refs    │  │ edges      │  │ Raw +     │ │
//! │  │ fts index    │  │            │  │ Summary   │ │
//! │  └──────────────┘  └────────────┘  │ Pointers  │ │
//! │                                     └──────────┘ │
//! └─────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Invariants
//!
//! 1. **Lossless**: Every message's original content is permanently retained
//!    in the immutable store, reachable via `lcm_grep` or `lcm_expand`.
//! 2. **Deterministic**: Context compaction is engine-driven using fixed
//!    three-level escalation, never delegated to the model.
//! 3. **Zero-Cost Continuity**: Below τ_soft, the store acts as a passive
//!    logger with no overhead.
//! 4. **DAG-structured summaries**: Summary nodes form a directed acyclic
//!    graph, allowing multi-resolution traversal of conversation history.

pub mod circuit_breaker;
pub mod compaction;
pub mod dag;
pub mod engine;
pub mod focus_briefs;
pub mod integrity;
pub mod repair;
pub mod spend_guard;
pub mod store;
pub mod summarizer;
pub mod tools;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;

pub use compaction::{NoopSummarizer, Summarizer};
pub use engine::LcmEngine;
pub use store::LcmStore;
pub use summarizer::ProviderSummarizer;
pub use tools::{LcmDescribeTool, LcmExpandTool, LcmGrepTool, LlmMapTool};
pub use types::*;

use crate::error::{AgentJaxError, AgentJaxResult};

/// Return the path to the LCM SQLite database for a conversation.
///
/// The database lives in the agent-scoped session directory:
/// `~/.agentjax/agents/{agent_id}/sessions/{conversation_id}/lcm.db`
pub fn lcm_store_path(agent_id: &str, conversation_id: &str) -> AgentJaxResult<PathBuf> {
    let dir = crate::conversation_store::conversation_workspace_path(agent_id, conversation_id)
        .map_err(|e| AgentJaxError::internal(format!("Failed to get workspace path: {e}")))?
        .parent()
        .ok_or_else(|| {
            AgentJaxError::not_found(format!(
                "Invalid conversation workspace path for '{conversation_id}'"
            ))
        })?
        .to_path_buf();
    // Directory creation is handled by LcmStore::open().
    Ok(dir.join(crate::conversation_store::paths::LCM_DB_FILE_NAME))
}

/// Open (or create) the LCM store and engine for a conversation.
///
/// Uses `NoopSummarizer` by default.
/// Currently only used in tests; production uses `open_lcm_engine_with_summarizer`.
#[cfg(test)]
pub fn open_lcm_engine(
    agent_id: &str,
    conversation_id: &str,
    lcm_config: &LcmConfig,
) -> AgentJaxResult<Arc<LcmEngine>> {
    let db_path = lcm_store_path(agent_id, conversation_id)?;
    let store = Arc::new(
        LcmStore::open(&db_path, lcm_config.clone())
            .map_err(|e| AgentJaxError::internal(format!("Failed to open LCM store: {e}")))?,
    );

    let engine = Arc::new(LcmEngine::new(
        store,
        Arc::new(NoopSummarizer),
        lcm_config.clone(),
    ));

    // Spawn background compaction task (async, non-blocking).
    engine.spawn_compaction_task();

    Ok(engine)
}

/// Open the LCM engine with a real provider-backed summarizer.
///
/// Resolves the summarization model from `LcmConfig.summarization_model`
/// (or falls back to `agent_config.utility_small_model`).
pub fn open_lcm_engine_with_summarizer(
    agent_id: &str,
    conversation_id: &str,
    lcm_config: &LcmConfig,
    app_config: &crate::config::AppConfig,
    agent_config: &crate::config::AgentConfig,
) -> AgentJaxResult<Arc<LcmEngine>> {
    let db_path = lcm_store_path(agent_id, conversation_id)?;
    let store = Arc::new(
        LcmStore::open(&db_path, lcm_config.clone())
            .map_err(|e| AgentJaxError::internal(format!("Failed to open LCM store: {e}")))?,
    );

    // Resolve the tokenizer model ID for accurate LCM token counting.
    let mut resolved_lcm_config = lcm_config.clone();
    if resolved_lcm_config.tokenizer_model_id.is_none() {
        // Use the summarization model for token counting, falling back to
        // the utility small model, then to the first configured model.
        let tokenizer_model: Option<String> = if !lcm_config.summarization_model.is_empty()
            && lcm_config.summarization_model != "default"
        {
            Some(lcm_config.summarization_model.clone())
        } else {
            let model = &agent_config.utility_small_model;
            if model.is_empty() {
                None
            } else {
                Some(model.clone())
            }
        };
        resolved_lcm_config.tokenizer_model_id = tokenizer_model;
    }

    // Try to create a ProviderSummarizer; fall back to NoopSummarizer
    // if model resolution fails.
    let summarizer: Arc<dyn Summarizer> = match ProviderSummarizer::new(
        app_config,
        agent_config,
        lcm_config,
    ) {
        Ok(ps) => {
            log::info!("LCM using ProviderSummarizer (model: {})", ps.model_ref());
            Arc::new(ps)
        }
        Err(e) => {
            log::warn!(
                "LCM: ProviderSummarizer unavailable ({}), falling back to NoopSummarizer (Level 3 truncation only)",
                e
            );
            Arc::new(NoopSummarizer)
        }
    };

    let engine = Arc::new(LcmEngine::new(store, summarizer, resolved_lcm_config));

    // Spawn background compaction task (async, non-blocking).
    engine.spawn_compaction_task();

    Ok(engine)
}

/// Convert a single `ConversationLine` to the fields needed for a `StoredMessage`.
///
/// Returns `(original_id, role, content, timestamp_unix_ms, metadata)`.
/// The caller is responsible for generating the final `MessageId`, computing
/// `token_count`, and setting `covered_by`, `thinking`, and `file_refs`.
pub fn conversation_line_to_stored_message_parts(
    line: &crate::conversation_store::ConversationLine,
) -> (String, types::MessageRole, String, i64, std::collections::BTreeMap<String, serde_json::Value>)
{
    use crate::conversation_store::{ConversationLine, ToolStatus};
    use crate::lcm::types::MessageRole;
    use std::collections::BTreeMap;

    match line {
        ConversationLine::User(u) => (
            u.id.clone(),
            MessageRole::User,
            u.text.clone(),
            u.ts,
            BTreeMap::from([(
                "request_id".to_string(),
                serde_json::Value::String(u.request_id.clone()),
            )]),
        ),
        ConversationLine::Assistant(a) => {
            let mut meta = BTreeMap::from([
                (
                    "request_id".to_string(),
                    serde_json::Value::String(a.request_id.clone()),
                ),
                (
                    "response_id".to_string(),
                    serde_json::Value::String(a.response_id.clone()),
                ),
            ]);
            if let Some(ref thinking) = a.thinking {
                meta.insert(
                    "thinking".to_string(),
                    serde_json::Value::String(thinking.clone()),
                );
            }
            if let Some(ref phase) = a.phase {
                meta.insert(
                    "phase".to_string(),
                    serde_json::Value::String(match phase {
                        crate::message_phase::AssistantPhase::Commentary => "commentary".to_string(),
                        crate::message_phase::AssistantPhase::FinalAnswer => "final_answer".to_string(),
                    }),
                );
            }
            (a.id.clone(), MessageRole::Assistant, a.text.clone(), a.ts, meta)
        }
        ConversationLine::Tool(t) => {
            let args_str = serde_json::to_string(&t.args).unwrap_or_else(|_| "{}".to_string());
            (
            t.id.clone(),
            MessageRole::Tool,
            t.output.as_ref().map(|o| o.to_string()).unwrap_or_default(),
            t.ts,
            BTreeMap::from([
                (
                    "request_id".to_string(),
                    serde_json::Value::String(t.request_id.clone()),
                ),
                (
                    "call_id".to_string(),
                    serde_json::Value::String(t.call_id.clone()),
                ),
                (
                    "tool_name".to_string(),
                    serde_json::Value::String(t.name.clone()),
                ),
                (
                    "args".to_string(),
                    serde_json::Value::String(args_str),
                ),
                (
                    "message_type".to_string(),
                    serde_json::Value::String(match t.status {
                        ToolStatus::Pending => "function_call".to_string(),
                        _ => "function_call_output".to_string(),
                    }),
                ),
            ]),
        )
        }
    }
}

/// Convert a single `StoredMessage` into provider API items.
///
/// For assistant messages with thinking content, this produces two items:
/// a `reasoning` block followed by the `output_text`. Tool messages are
/// converted to `function_call` / `function_call_output` items based on
/// message_type metadata. All other messages become role-based items with
/// `input_text` content.
///
/// This is the canonical implementation — both `InMemoryContext` and
/// `MemoryAgentContext` delegate to this to avoid code duplication with
/// `LcmEngine::context_to_provider_items`.
pub fn stored_message_to_provider_items(
    msg: &types::StoredMessage,
) -> Vec<serde_json::Value> {
    let mut items = Vec::with_capacity(2);

    match msg.role {
        types::MessageRole::User => {
            items.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "input_text", "text": &msg.content}]
            }));
        }
        types::MessageRole::Assistant => {
            // Inject thinking content if present.
            if let Some(ref t) = msg.thinking {
                let trimmed = t.trim();
                if !trimmed.is_empty() {
                    items.push(serde_json::json!({
                        "type": "reasoning",
                        "text": trimmed,
                    }));
                }
            }
            items.push(serde_json::json!({
                "role": "assistant",
                "content": [{"type": "input_text", "text": &msg.content}]
            }));
        }
        types::MessageRole::Tool => {
            // Structured tool messages via metadata.
            if let Some(msg_type) =
                msg.metadata.get("message_type").and_then(|v| v.as_str())
            {
                match msg_type {
                    "function_call" => {
                        let call_id = msg
                            .metadata
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let name = msg
                            .metadata
                            .get("tool_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let arguments = msg
                            .metadata
                            .get("arguments")
                            .and_then(|v| v.as_str())
                            .unwrap_or("{}");
                        items.push(serde_json::json!({
                            "type": "function_call",
                            "call_id": call_id,
                            "name": name,
                            "arguments": arguments,
                        }));
                    }
                    "function_call_output" => {
                        let call_id = msg
                            .metadata
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        items.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": &msg.content,
                        }));
                    }
                    _ => {
                        items.push(serde_json::json!({
                            "role": "user",
                            "content": [{"type": "input_text", "text": &msg.content}]
                        }));
                    }
                }
            } else {
                items.push(serde_json::json!({
                    "role": "user",
                    "content": [{"type": "input_text", "text": &msg.content}]
                }));
            }
        }
    }

    items
}

/// Convert LCM `StoredMessage`s into `ConversationLine`s for frontend display.
///
/// This bridges the LCM immutable store (single source of truth) to the
/// frontend's expected `ConversationDetail.lines` format. Fields not stored
/// in LCM (transient UI state, tool presentation metadata) are filled with
/// sensible defaults.
pub fn stored_messages_to_conversation_lines(
    messages: &[types::StoredMessage],
) -> Vec<crate::conversation_store::ConversationLine> {
    use crate::conversation_store::{
        AssistantLine, AssistantStatus, ConversationLine, ToolLine, ToolStatus, UserLine,
    };

    messages
        .iter()
        .filter_map(|msg| {
            let id = msg.id.to_string();
            let ts = msg.timestamp_unix_ms;
            let request_id = msg
                .metadata
                .get("request_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();

            match msg.role {
                types::MessageRole::User => Some(ConversationLine::User(UserLine {
                    id,
                    ts,
                    request_id,
                    text: msg.content.clone(),
                })),
                types::MessageRole::Assistant => {
                    let phase = msg
                        .metadata
                        .get("phase")
                        .and_then(|v| v.as_str())
                        .and_then(|s| match s {
                            "commentary" | "assistant" => {
                                // "assistant" is a legacy value from an older
                                // fallback path — treat it as commentary.
                                Some(crate::message_phase::AssistantPhase::Commentary)
                            }
                            "final" | "final_answer" => {
                                Some(crate::message_phase::AssistantPhase::FinalAnswer)
                            }
                            _ => None,
                        });
                    let response_id = msg
                        .metadata
                        .get("response_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let thinking = msg.thinking.clone();
                    let thinking_token_count = thinking
                        .as_ref()
                        .map(|t| crate::lcm::types::estimate_tokens(t));
                    Some(ConversationLine::Assistant(AssistantLine {
                        id,
                        ts,
                        request_id,
                        response_id,
                        phase,
                        text: msg.content.clone(),
                        thinking,
                        thinking_token_count,
                        status: AssistantStatus::Done,
                    }))
                }
                types::MessageRole::Tool => {
                    let message_type = msg
                        .metadata
                        .get("message_type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let call_id = msg
                        .metadata
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    let name = msg
                        .metadata
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();

                    match message_type {
                        "function_call" => {
                            // Prefer args from metadata; fall back to parsing content
                            // for records written before the args metadata key was added.
                            let args: serde_json::Value = msg
                                .metadata
                                .get("args")
                                .and_then(|v| v.as_str())
                                .and_then(|s| serde_json::from_str(s).ok())
                                .or_else(|| serde_json::from_str(&msg.content).ok())
                                .unwrap_or(serde_json::Value::Null);
                            Some(ConversationLine::Tool(ToolLine {
                                id: format!("tool-{request_id}-{call_id}"),
                                ts,
                                started_ts: ts,
                                completed_ts: None,
                                request_id,
                                call_id,
                                name,
                                display_name: None,
                                description: None,
                                icon: None,
                                args,
                                output: None,
                                status: ToolStatus::Pending,
                            }))
                        }
                        "function_call_output" => {
                            let output: serde_json::Value = serde_json::from_str(&msg.content)
                                .unwrap_or(serde_json::Value::Null);
                            // Prefer args from metadata; fall back to Null for
                            // records written before the args metadata key was added.
                            let args: serde_json::Value = msg
                                .metadata
                                .get("args")
                                .and_then(|v| v.as_str())
                                .and_then(|s| serde_json::from_str(s).ok())
                                .unwrap_or(serde_json::Value::Null);
                            let is_error = output.get("ok").and_then(|v| v.as_bool())
                                == Some(false)
                                || output.get("error").is_some();
                            Some(ConversationLine::Tool(ToolLine {
                                id: format!("tool-{request_id}-{call_id}"),
                                ts,
                                started_ts: 0,
                                completed_ts: Some(ts),
                                request_id,
                                call_id,
                                name,
                                display_name: None,
                                description: None,
                                icon: None,
                                args,
                                output: Some(output),
                                status: if is_error {
                                    ToolStatus::Failed
                                } else {
                                    ToolStatus::Done
                                },
                            }))
                        }
                        _ => None,
                    }
                }
            }
        })
        .collect()
}

// ── JSONL → LCM Backfill ────────────────────────────────────────────────

/// Maximum tokens to import from JSONL during backfill.
/// This prevents flooding the LCM store with huge pre-LCM conversations.
pub const BACKFILL_MAX_TOKENS: u32 = 200_000;

/// Backfill the LCM store from the JSONL session file.
///
/// Called when a conversation exists (has metadata.json / messages.jsonl)
/// but the LCM database is empty or unpopulated. Reads messages from JSONL
/// and imports them into the LCM SQLite store so subsequent loads use LCM.
///
/// This is a one-time migration: after backfill, the LCM store becomes the
/// single source of truth. JSONL remains as a backup.
///
/// Returns `true` if LCM was populated, `false` if no JSONL data was found
/// or if LCM already had data.
pub fn backfill_lcm_from_jsonl(
    agent_id: &str,
    conversation_id: &str,
) -> Result<bool, crate::error::AgentJaxError> {
    // ── Step 1: Check LCM DB — skip if already populated ──
    let db_path = lcm_store_path(agent_id, conversation_id)
        .map_err(|e| crate::error::AgentJaxError::internal(format!("Bad store path: {e}")))?;

    if db_path.exists() {
        // DB exists — check if it has messages.
        if let Ok(store) = crate::lcm::LcmStore::open(&db_path, crate::lcm::LcmConfig::default()) {
            if let Ok(messages) = store.get_conversation_messages(conversation_id) {
                if !messages.is_empty() {
                    return Ok(false); // Already populated.
                }
            }
        }
    }

    // ── Step 2: Read JSONL session file ──
    let messages_path = match crate::conversation_store::conversation_dir_path(
        agent_id,
        conversation_id,
    ) {
        Ok(d) => d.join(crate::conversation_store::paths::MESSAGES_FILE_NAME),
        Err(_) => return Ok(false),
    };

    if !messages_path.exists() {
        return Ok(false); // No JSONL file to import from.
    }

    let jsonl_content = match std::fs::read_to_string(&messages_path) {
        Ok(c) => c,
        Err(_) => return Ok(false),
    };

    let jsonl_lines: Vec<&str> = jsonl_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();

    if jsonl_lines.is_empty() {
        return Ok(false);
    }

    // ── Step 3: Open or create LCM store ──
    let store =
        crate::lcm::LcmStore::open(&db_path, crate::lcm::LcmConfig::default()).map_err(|e| {
            crate::error::AgentJaxError::internal(format!("Cannot open LCM store: {e}"))
        })?;

    // ── Step 4: Parse JSONL and convert to StoredMessages ──
    use crate::lcm::types::{LcmId, StoredMessage};

    let mut stored_messages: Vec<StoredMessage> = Vec::new();
    let mut total_tokens: u32 = 0;

    for line in &jsonl_lines {
        let Ok(cl) = serde_json::from_str::<crate::conversation_store::ConversationLine>(line)
        else {
            continue;
        };

        let (_line_id, role, content, ts, metadata_map) =
            conversation_line_to_stored_message_parts(&cl);

        let msg_id = LcmId::new();
        let token_count = crate::lcm::types::estimate_tokens(&content);

        // Check token budget.
        if total_tokens + token_count > BACKFILL_MAX_TOKENS {
            log::warn!(
                "LCM backfill: reached token budget ({}/{}) after {} messages",
                total_tokens,
                BACKFILL_MAX_TOKENS,
                stored_messages.len()
            );
            break;
        }

        total_tokens += token_count;
        stored_messages.push(StoredMessage {
            id: msg_id,
            conversation_id: conversation_id.to_string(),
            role,
            content,
            token_count,
            timestamp_unix_ms: ts,
            covered_by: None,
            thinking: None,
            metadata: metadata_map,
            file_refs: Vec::new(),
        });
    }

    if stored_messages.is_empty() {
        return Ok(false);
    }

    // ── Step 5: Persist to LCM store ──
    store.persist_messages(&stored_messages).map_err(|e| {
        crate::error::AgentJaxError::internal(format!("Failed to persist backfill: {e}"))
    })?;

    log::info!(
        "LCM backfill: imported {} messages ({} tokens) for '{}' from JSONL",
        stored_messages.len(),
        total_tokens,
        conversation_id,
    );

    Ok(true)
}

// ── Smoke tests: JSONL-optional conversations ───────────────────────────

#[cfg(test)]
mod smoke_tests {
    use super::*;
    use crate::conversation_store::{AssistantStatus, ConversationLine, ToolStatus};
    use serde_json::json;
    use std::collections::BTreeMap;

    fn make_test_store() -> (std::path::PathBuf, Arc<LcmStore>) {
        let dir = std::env::temp_dir().join(format!("lcm-smoke-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("create test dir");
        let db_path = dir.join("lcm.db");
        let store =
            Arc::new(LcmStore::open(&db_path, LcmConfig::default()).expect("open LCM store"));
        (dir, store)
    }

    fn make_stored_message(
        id: &str,
        conv_id: &str,
        role: types::MessageRole,
        content: &str,
        ts: i64,
        metadata: BTreeMap<String, serde_json::Value>,
    ) -> types::StoredMessage {
        types::StoredMessage {
            id: types::MessageId::from(id),
            conversation_id: conv_id.to_string(),
            role,
            content: content.to_string(),
            token_count: types::estimate_tokens(content),
            timestamp_unix_ms: ts,
            covered_by: None,
            metadata,
            file_refs: Vec::new(),
            thinking: None,
        }
    }

    #[test]
    fn smoke_user_message_roundtrips() {
        let msg = make_stored_message(
            "user-1",
            "conv-1",
            types::MessageRole::User,
            "Hello, world!",
            1000,
            BTreeMap::from([("request_id".to_string(), json!("req-1"))]),
        );
        let lines = stored_messages_to_conversation_lines(&[msg]);
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            ConversationLine::User(u) => {
                assert_eq!(u.text, "Hello, world!");
                assert_eq!(u.request_id, "req-1");
            }
            _ => panic!("Expected User line, got {:?}", lines[0]),
        }
    }

    #[test]
    fn smoke_assistant_message_roundtrips() {
        let mut meta = BTreeMap::new();
        meta.insert("request_id".to_string(), json!("req-1"));
        meta.insert("response_id".to_string(), json!("resp-1"));
        meta.insert("phase".to_string(), json!("final"));
        let msg = make_stored_message(
            "asst-1",
            "conv-1",
            types::MessageRole::Assistant,
            "The answer is 42.",
            2000,
            meta,
        );
        let lines = stored_messages_to_conversation_lines(&[msg]);
        assert_eq!(lines.len(), 1);
        match &lines[0] {
            ConversationLine::Assistant(a) => {
                assert_eq!(a.text, "The answer is 42.");
                assert!(a.phase.is_some());
                assert!(matches!(a.status, AssistantStatus::Done));
            }
            _ => panic!("Expected Assistant line"),
        }
    }

    #[test]
    fn smoke_tool_call_and_result_roundtrip() {
        let conv_id = "conv-tool-test";

        // Simulate function_call
        let mut fc_meta = BTreeMap::new();
        fc_meta.insert("request_id".to_string(), json!("req-1"));
        fc_meta.insert("message_type".to_string(), json!("function_call"));
        fc_meta.insert("call_id".to_string(), json!("call-123"));
        fc_meta.insert("tool_name".to_string(), json!("calculator"));
        let fc_msg = make_stored_message(
            "fc-1",
            conv_id,
            types::MessageRole::Tool,
            r#"{"expression": "2+2"}"#,
            3000,
            fc_meta,
        );

        // Simulate function_call_output
        let mut fco_meta = BTreeMap::new();
        fco_meta.insert("request_id".to_string(), json!("req-1"));
        fco_meta.insert("message_type".to_string(), json!("function_call_output"));
        fco_meta.insert("call_id".to_string(), json!("call-123"));
        fco_meta.insert("tool_name".to_string(), json!("calculator"));
        let fco_msg = make_stored_message(
            "fco-1",
            conv_id,
            types::MessageRole::Tool,
            r#"{"result":"4"}"#,
            4000,
            fco_meta,
        );

        let lines = stored_messages_to_conversation_lines(&[fc_msg, fco_msg]);
        assert_eq!(lines.len(), 2);

        // First line should be the tool call (Pending, with args)
        match &lines[0] {
            ConversationLine::Tool(t) => {
                assert_eq!(t.call_id, "call-123");
                assert_eq!(t.name, "calculator");
                assert!(matches!(t.status, ToolStatus::Pending));
            }
            _ => panic!("Expected Tool line (call), got {:?}", lines[0]),
        }

        // Second line should be the tool result (Done, with output)
        match &lines[1] {
            ConversationLine::Tool(t) => {
                assert_eq!(t.call_id, "call-123");
                assert_eq!(t.name, "calculator");
                assert!(matches!(t.status, ToolStatus::Done));
                assert!(t.output.is_some());
            }
            _ => panic!("Expected Tool line (result), got {:?}", lines[1]),
        }
    }

    #[test]
    fn smoke_full_conversation_roundtrip() {
        let conv_id = "conv-full-test";

        let messages = vec![
            make_stored_message(
                "u-1",
                conv_id,
                types::MessageRole::User,
                "Hi",
                1000,
                BTreeMap::from([("request_id".to_string(), json!("r1"))]),
            ),
            make_stored_message(
                "a-1",
                conv_id,
                types::MessageRole::Assistant,
                "Hello!",
                2000,
                BTreeMap::from([
                    ("request_id".to_string(), json!("r1")),
                    ("response_id".to_string(), json!("resp1")),
                    ("phase".to_string(), json!("final")),
                ]),
            ),
        ];

        let lines = stored_messages_to_conversation_lines(&messages);
        assert_eq!(lines.len(), 2);
        assert!(matches!(lines[0], ConversationLine::User(_)));
        assert!(matches!(lines[1], ConversationLine::Assistant(_)));
    }

    #[test]
    fn smoke_conversation_loads_from_lcm_only_no_jsonl() {
        let (dir, store) = make_test_store();
        let conv_id = "conv-lcm-only";

        // Persist messages directly to LCM (no JSONL).
        let messages = vec![
            make_stored_message(
                "u-1",
                conv_id,
                types::MessageRole::User,
                "Hello",
                1000,
                BTreeMap::from([("request_id".to_string(), json!("r1"))]),
            ),
            make_stored_message(
                "a-1",
                conv_id,
                types::MessageRole::Assistant,
                "Hi there!",
                2000,
                BTreeMap::from([
                    ("request_id".to_string(), json!("r1")),
                    ("response_id".to_string(), json!("resp1")),
                    ("phase".to_string(), json!("final")),
                ]),
            ),
        ];
        store.persist_messages(&messages).expect("persist");

        // Read back and convert.
        let loaded = store.get_conversation_messages(conv_id).expect("load");
        assert_eq!(loaded.len(), 2);

        let lines = stored_messages_to_conversation_lines(&loaded);
        assert_eq!(lines.len(), 2);
        assert!(matches!(lines[0], ConversationLine::User(_)));
        assert!(matches!(lines[1], ConversationLine::Assistant(_)));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn smoke_lcm_store_persists_and_retrieves_metadata() {
        let (dir, store) = make_test_store();
        let conv_id = "conv-meta-test";

        // Ensure conversation meta
        let meta = store.ensure_conversation_meta(conv_id).expect("ensure");
        assert_eq!(meta.conversation_id, conv_id);

        // Update title
        store
            .update_conversation_meta(conv_id, Some("Test Title"), Some("manual"), None, None)
            .expect("update");

        let updated = store.get_conversation_meta(conv_id).expect("get");
        assert_eq!(updated.unwrap().title, "Test Title");

        let _ = std::fs::remove_dir_all(&dir);
    }
}
