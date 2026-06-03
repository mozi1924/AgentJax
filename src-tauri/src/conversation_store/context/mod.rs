//! Conversation context assembly pipeline.
//!
//! This module turns persisted conversation lines into Responses-API input
//! items and keeps the transformation steps separated so new context rules can
//! be added without expanding one oversized file.

mod builders;
mod budget;
mod policy;
mod sanitizer;
mod token_usage;
mod truncation;
mod types;

#[cfg(test)]
mod tests;

use super::file_io::read_conversation_file;
use super::locks::with_conversation_lock;
use super::paths::{conversation_messages_path, conversation_metadata_path};
use builders::build_context_items;
use policy::MAX_CONTEXT_ITEMS_PER_REQUEST;
use sanitizer::sanitize_tool_call_pairs;
use truncation::truncate_context_items_preserving_tool_pairs;

pub use budget::{TokenBudget, truncate_items_to_budget, estimate_input_items_tokens};
pub use token_usage::{
    ConversationTokenUsage, TokenCountFunctionCall, TokenCountMessage,
    count_conversation_context_tokens, count_conversation_prompt_tokens, count_messages_tokens,
    count_request_prompt_tokens, count_text_tokens, count_tool_schema_tokens,
};
pub use types::ConversationContext;

/// Load a conversation snapshot and convert it into model-ready input items.
///
/// Reads from LCM immutable store first; falls back to legacy JSONL.
///
/// When `budget` is provided, the context is truncated to fit the model's
/// token budget in addition to the hard item-count limit. If `budget` is
/// `None`, only the hard count limit is applied.
pub fn load_context_for_request(
    conversation_id: &str,
    budget: Option<&TokenBudget>,
) -> crate::error::AgentJaxResult<ConversationContext> {
    with_conversation_lock(conversation_id, || {
        // ── Try LCM immutable store first ──────────────────────────
        if let Ok(Some(ctx)) = load_context_from_lcm(conversation_id, budget) {
            return Ok(ctx);
        }

        // ── Fall back to legacy JSONL ──────────────────────────────
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let messages_path = conversation_messages_path(conversation_id)?;
        let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
            return Ok(ConversationContext::default());
        };

        let mut input_items = build_context_items(&data.lines);
        input_items = sanitize_tool_call_pairs(input_items);
        input_items = truncate_context_items_preserving_tool_pairs(
            input_items,
            MAX_CONTEXT_ITEMS_PER_REQUEST,
        );
        if let Some(budget) = budget {
            input_items = truncate_items_to_budget(input_items, budget);
        }

        let estimated_tokens = estimate_input_items_tokens(&input_items);
        let tool_call_count = input_items
            .iter()
            .filter(|item| {
                item.get("type")
                    .and_then(|v| v.as_str())
                    .map(|t| t == "function_call" || t == "function_call_output")
                    .unwrap_or(false)
            })
            .count();

        Ok(ConversationContext {
            input_items,
            estimated_tokens,
            tool_call_count,
            message_count: data.lines.len(),
        })
    })
}

/// Load context from LCM store and convert to input items.
fn load_context_from_lcm(
    conversation_id: &str,
    budget: Option<&TokenBudget>,
) -> crate::error::AgentJaxResult<Option<ConversationContext>> {
    use crate::conversation_store::paths::conversation_lcm_db_path;

    let db_path = conversation_lcm_db_path(conversation_id)?;
    if !db_path.exists() {
        return Ok(None);
    }

    let lcm_config = crate::lcm::LcmConfig::default();
    let store = crate::lcm::LcmStore::open(&db_path, lcm_config)
        .map_err(|e| format!("Failed to open LCM store for context: {e}"))?;

    let messages = store
        .get_conversation_messages(conversation_id)
        .map_err(|e| format!("Failed to read LCM messages: {e}"))?;

    if messages.is_empty() {
        return Ok(None);
    }

    // Convert StoredMessages to ConversationLines, then to input items.
    let lines = crate::lcm::stored_messages_to_conversation_lines(&messages);
    let mut input_items = build_context_items(&lines);
    input_items = sanitize_tool_call_pairs(input_items);
    input_items = truncate_context_items_preserving_tool_pairs(
        input_items,
        MAX_CONTEXT_ITEMS_PER_REQUEST,
    );
    if let Some(budget) = budget {
        input_items = truncate_items_to_budget(input_items, budget);
    }

    let estimated_tokens = estimate_input_items_tokens(&input_items);
    let tool_call_count = input_items
        .iter()
        .filter(|item| {
            item.get("type")
                .and_then(|v| v.as_str())
                .map(|t| t == "function_call" || t == "function_call_output")
                .unwrap_or(false)
        })
        .count();

    Ok(Some(ConversationContext {
        input_items,
        estimated_tokens,
        tool_call_count,
        message_count: lines.len(),
    }))
}
