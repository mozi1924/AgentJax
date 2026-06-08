//! Context assembly — builds the input items for each hop of the turn loop.
//!
//! Extracted from the monolithic `run_turn()` in `engine.rs`.

use crate::runtime::agent_context::AgentContext;
use crate::error::AgentJaxResult;
use serde_json::Value;

/// Build the hop prefix (system items + recovery note + street notifications).
pub(crate) fn build_hop_prefix(
    system_items: Vec<Value>,
    recovery_note: Option<Value>,
    street_items: Vec<Value>,
) -> Vec<Value> {
    let mut prefix = system_items;
    if let Some(note) = recovery_note {
        prefix.push(note);
    }
    // Inject Street notifications as user-role items.
    // We use user role (not system) to avoid prompt injection risks
    // from dynamic async result content — the model treats these as
    // data/observations rather than authoritative instructions.
    prefix.extend(street_items);
    prefix
}

fn get_user_item_text(item: &Value) -> Option<&str> {
    if item.get("role").and_then(|v| v.as_str()) != Some("user") {
        return None;
    }
    if let Some(content) = item.get("content") {
        if let Some(arr) = content.as_array() {
            if let Some(first) = arr.first() {
                if first.get("type").and_then(|v| v.as_str()) == Some("input_text") {
                    return first.get("text").and_then(|v| v.as_str());
                }
            }
        } else if let Some(text) = content.as_str() {
            return Some(text);
        }
    }
    None
}

/// Build the input items for hop 1 of the turn loop.
///
/// Hop 1 includes: prefix + LCM history + rendered current user message.
/// For auto-resume, all LCM context is included as-is.
/// For normal turns, the most recent user message is replaced with a
/// timestamped "Current user message" rendering.
pub(crate) async fn build_hop1_input(
    hop_prefix: &[Value],
    context: &dyn AgentContext,
    accumulated_context: &[Value],
    is_auto_resume: bool,
    provider_kind: &str,
    user_message_ts: i64,
    user_input: &str,
) -> AgentJaxResult<Vec<Value>> {
    let active_context = context.context_items().await?;
    let lcm_context = if active_context.is_empty() {
        accumulated_context.to_vec()
    } else {
        active_context
    };

    if is_auto_resume {
        let mut items = hop_prefix.to_vec();
        items.extend(lcm_context);
        return Ok(items);
    }

    // Keep all historical user messages — the model needs to see what the
    // user previously asked. Only the very last user item is skipped because
    // it will be re-rendered below with a "Current user message" label.
    // We only pop it if it matches the current user input to prevent history
    // corruption if the message was not successfully persisted yet.
    // Use rposition() to find the most recent matching user message anywhere
    // in the context (not just the last element), since LCM compression or
    // other restructuring may reorder entries.
    let mut history_items = lcm_context;
    if let Some(pos) = history_items
        .iter()
        .rposition(|item| get_user_item_text(item).map_or(false, |t| t.trim() == user_input.trim()))
    {
        history_items.remove(pos);
    }

    let mut items = hop_prefix.to_vec();
    items.extend(history_items);
    items.push(crate::provider_api::build_user_input_item(
        provider_kind,
        &crate::time_context::render_timed_message(
            "Current user message",
            user_message_ts,
            user_input.trim(),
        ),
    )?);
    Ok(items)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hop_prefix_includes_system_recovery_and_street() {
        let prefix = build_hop_prefix(
            vec![serde_json::json!({"role":"system","content":"sys"})],
            Some(serde_json::json!({"role":"system","content":"recovery"})),
            vec![serde_json::json!({"role":"user","content":"street"})],
        );
        assert_eq!(prefix.len(), 3);
        assert_eq!(prefix[0]["role"], "system");
        assert_eq!(prefix[1]["role"], "system");
        assert_eq!(prefix[2]["role"], "user"); // street as user-role
    }

    #[tokio::test]
    async fn test_build_hop1_input_skips_matching_user_message() {
        use crate::runtime::agent_context::{AgentContext, InMemoryContext};
        use crate::lcm::types::{StoredMessage, MessageRole};
        
        let ctx = InMemoryContext::new();
        ctx.persist_message(&StoredMessage {
            id: crate::lcm::types::MessageId::new(),
            conversation_id: "test-conv".to_string(),
            role: MessageRole::User,
            content: "Hello".to_string(),
            token_count: 1,
            timestamp_unix_ms: 1000,
            covered_by: None,
            thinking: None,
            seq: 1,
            hop_index: 0,
            metadata: std::collections::BTreeMap::new(),
            file_refs: Vec::new(),
        }).await.unwrap();

        let prefix = vec![];
        let res = build_hop1_input(&prefix, &ctx, &[], false, "openai", 1000, "Hello").await.unwrap();
        assert_eq!(res.len(), 1);
        assert!(res[0]["content"][0]["text"].as_str().unwrap().contains("Hello"));
        assert!(res[0]["content"][0]["text"].as_str().unwrap().contains("Current user message"));
    }

    #[tokio::test]
    async fn test_build_hop1_input_keeps_non_matching_user_message() {
        use crate::runtime::agent_context::{AgentContext, InMemoryContext};
        use crate::lcm::types::{StoredMessage, MessageRole};
        
        let ctx = InMemoryContext::new();
        ctx.persist_message(&StoredMessage {
            id: crate::lcm::types::MessageId::new(),
            conversation_id: "test-conv".to_string(),
            role: MessageRole::User,
            content: "Old message".to_string(),
            token_count: 1,
            timestamp_unix_ms: 1000,
            covered_by: None,
            thinking: None,
            seq: 1,
            hop_index: 0,
            metadata: std::collections::BTreeMap::new(),
            file_refs: Vec::new(),
        }).await.unwrap();

        let prefix = vec![];
        let res = build_hop1_input(&prefix, &ctx, &[], false, "openai", 1000, "New input").await.unwrap();
        assert_eq!(res.len(), 2);
        assert_eq!(res[0]["content"][0]["text"].as_str().unwrap(), "Old message");
        assert!(res[1]["content"][0]["text"].as_str().unwrap().contains("New input"));
    }
}
