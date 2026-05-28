//! Conversation context assembly pipeline.
//!
//! This module turns persisted conversation lines into Responses-API input
//! items and keeps the transformation steps separated so new context rules can
//! be added without expanding one oversized file.

mod builders;
mod policy;
mod sanitizer;
mod truncation;
mod types;
mod token_usage;

#[cfg(test)]
mod tests;

use super::file_io::read_conversation_file;
use super::locks::with_conversation_lock;
use super::paths::{conversation_messages_path, conversation_metadata_path};
use builders::build_context_items;
use policy::MAX_CONTEXT_ITEMS_PER_REQUEST;
use sanitizer::sanitize_tool_call_pairs;
use truncation::truncate_context_items_preserving_tool_pairs;

pub use types::ConversationContext;
pub use token_usage::{
    count_conversation_context_tokens, count_conversation_prompt_tokens,
    count_messages_tokens, count_request_prompt_tokens, count_tool_schema_tokens,
    ConversationTokenUsage,
};

/// Load a conversation snapshot and convert it into model-ready input items.
///
/// The lock is held across the file read and transformation steps so the
/// resulting context stays consistent with the persisted conversation state.
pub fn load_context_for_request(conversation_id: &str) -> Result<ConversationContext, String> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let messages_path = conversation_messages_path(conversation_id)?;
        let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
            return Ok(ConversationContext::default());
        };

        let mut input_items = build_context_items(&data.lines);

        // Drop unmatched tool entries before we apply the request budget.
        input_items = sanitize_tool_call_pairs(input_items);
        input_items = truncate_context_items_preserving_tool_pairs(
            input_items,
            MAX_CONTEXT_ITEMS_PER_REQUEST,
        );

        Ok(ConversationContext { input_items })
    })
}
