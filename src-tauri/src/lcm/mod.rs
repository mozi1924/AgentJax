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

pub mod compaction;
pub mod dag;
pub mod engine;
pub mod file_handler;
pub mod store;
pub mod tools;
pub mod types;

use std::path::PathBuf;
use std::sync::Arc;

pub use compaction::{CompactionEngine, NoopSummarizer, Summarizer};
pub use dag::SummaryDag;
pub use engine::LcmEngine;
pub use file_handler::FileHandler;
pub use store::LcmStore;
pub use tools::{LcmDescribeTool, LcmExpandTool, LcmGrepTool};
pub use types::*;

/// Return the path to the LCM SQLite database for a conversation.
///
/// The database lives alongside the existing JSONL messages file:
/// `~/.agentjax/sessions/{conversation_id}/lcm.db`
pub fn lcm_store_path(conversation_id: &str) -> Result<PathBuf, String> {
    let dir = crate::conversation_store::conversation_workspace_path(conversation_id)?
        .parent()
        .ok_or_else(|| format!("Invalid conversation workspace path for '{}'", conversation_id))?
        .to_path_buf();
    // Ensure the directory exists.
    std::fs::create_dir_all(&dir).map_err(|e| {
        format!(
            "Failed to create LCM store directory {}: {e}",
            dir.display()
        )
    })?;
    Ok(dir.join("lcm.db"))
}

/// Open (or create) the LCM store and engine for a conversation.
///
/// Returns `None` if LCM is disabled in the configuration.
pub fn open_lcm_engine(
    conversation_id: &str,
    config: &LcmConfig,
) -> Result<Option<Arc<LcmEngine>>, String> {
    if !config.enabled {
        return Ok(None);
    }

    let db_path = lcm_store_path(conversation_id)?;
    let store = Arc::new(
        LcmStore::open(&db_path, config.clone())
            .map_err(|e| format!("Failed to open LCM store: {e}"))?,
    );

    // Use NoopSummarizer for now — LLM-powered summarization will be
    // wired in a future phase when the provider API is integrated.
    let engine = Arc::new(LcmEngine::new(
        store,
        Arc::new(NoopSummarizer),
        config.clone(),
    ));

    Ok(Some(engine))
}

/// Sync messages from the legacy ConversationStore to the LCM immutable store.
///
/// This is a bridge function that reads the conversation from the existing
/// JSONL-based store and mirrors each message into the SQLite-backed LCM
/// store. Messages already present (matched by ID) are skipped.
///
/// This function is called after each `chat_stream` turn completes to keep
/// the LCM store in sync with the canonical conversation state.
pub async fn sync_conversation_to_lcm(
    conversation_id: &str,
    lcm_store: &Arc<LcmStore>,
) -> Result<(), String> {
    use crate::conversation_store::ConversationLine;

    let detail = crate::conversation_store::load_conversation(conversation_id)
        .map_err(|e| format!("Failed to load conversation for LCM sync: {e}"))?;

    let Some(detail) = detail else {
        return Ok(()); // Conversation doesn't exist yet.
    };

    let now_ms = crate::conversation_store_utils::now_unix_ms();

    for line in &detail.lines {
        let (lcm_id, role, content, ts) = match line {
            ConversationLine::User(user) => (
                types::MessageId::from(user.id.as_str()),
                types::MessageRole::User,
                user.text.clone(),
                user.ts,
            ),
            ConversationLine::Assistant(asst) => (
                types::MessageId::from(asst.id.as_str()),
                types::MessageRole::Assistant,
                asst.text.clone(),
                asst.ts,
            ),
            ConversationLine::Tool(tool) => (
                types::MessageId::from(tool.id.as_str()),
                types::MessageRole::Tool,
                format_tool_line_content(tool),
                tool.ts,
            ),
        };

        let token_count = types::estimate_tokens(&content);
        let msg = types::StoredMessage::new(
            lcm_id,
            conversation_id,
            role,
            content,
            token_count,
            ts,
        );

        // Use INSERT OR IGNORE to skip already-persisted messages.
        lcm_store
            .persist_message(&msg)
            .map_err(|e| format!("Failed to persist message to LCM: {e}"))?;
    }

    let _ = now_ms; // silence unused warning

    log::debug!(
        "LCM sync complete for conversation '{}': {} messages",
        conversation_id,
        detail.lines.len()
    );

    Ok(())
}

fn format_tool_line_content(tool: &crate::conversation_store::ToolLine) -> String {
    let args_str = serde_json::to_string(&tool.args).unwrap_or_else(|_| "{}".to_string());

    let output_str = match &tool.output {
        Some(output) => format!(" → {}", output),
        None => String::from(" (pending)"),
    };

    format!(
        "[Tool: {}] args={}{}",
        tool.name, args_str, output_str
    )
}
