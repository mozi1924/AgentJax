//! LCM persistence helpers — persist assistant messages, thinking content,
//! tool calls, and tool results to the LCM store during a turn.
//!
//! Extracted from the monolithic `run_turn()` in `engine.rs`.


use crate::lcm::types::{self as lcm, StoredMessage};
use crate::message_phase::AssistantPhase;
use crate::runtime::agent_context::AgentContext;
use serde_json::Value;
use std::collections::BTreeMap;

/// Persist assistant messages for a single hop, including thinking content
/// and embedded tool calls.
pub(crate) async fn persist_hop_assistant_messages(
    context: &dyn AgentContext,
    conversation_id: &str,
    request_id: &str,
    response_id: &str,
    turn_idx: usize,
    hop_messages_for_lcm: &[(String, Option<AssistantPhase>)],
    output_items: &[Value],
    output_text: &str,
    is_final_hop: bool,
    final_output_text: &str,
) {
    let now_ms = crate::conversation_store_utils::now_unix_ms();
    let mut batch_messages: Vec<StoredMessage> = Vec::new();

    // ── Assistant text messages ──────────────────────────────────────────
    for (text, phase) in hop_messages_for_lcm {
        if !text.trim().is_empty() {
            let mut msg = StoredMessage::new(
                lcm::MessageId::new(),
                conversation_id,
                lcm::MessageRole::Assistant,
                text,
                lcm::estimate_tokens(text),
                now_ms,
                0,
                turn_idx as u32,
            );
            msg.metadata.insert(
                "request_id".to_string(),
                Value::String(request_id.to_string()),
            );
            msg.metadata.insert(
                "response_id".to_string(),
                Value::String(response_id.to_string()),
            );
            if let Some(p) = phase {
                msg.metadata
                    .insert("phase".to_string(), Value::String(p.as_str().to_string()));
            }
            batch_messages.push(msg);
        }
    }

    // ── Extract reasoning/thinking from output_items ─────────────────────
    let hop_thinking_text: Option<String> = {
        let parts: Vec<&str> = output_items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
            .filter_map(|item| item.get("text").and_then(Value::as_str))
            .filter(|text| !text.trim().is_empty())
            .collect();
        if parts.is_empty() {
            None
        } else {
            Some(parts.join("\n"))
        }
    };

    // Attach thinking to the first assistant message.
    if let Some(ref thinking_text) = hop_thinking_text
        && let Some(msg) = batch_messages
            .iter_mut()
            .find(|m| m.role == lcm::MessageRole::Assistant)
        {
            msg.thinking = Some(thinking_text.clone());
        }

    // ── Embed tool calls in the first assistant message ──────────────────
    {
        let tool_calls: Vec<Value> = output_items
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("function_call"))
            .cloned()
            .collect();
        if !tool_calls.is_empty()
            && let Some(msg) = batch_messages
                .iter_mut()
                .find(|m| m.role == lcm::MessageRole::Assistant)
            {
                msg.metadata.insert(
                    "tool_calls_json".to_string(),
                    Value::String(serde_json::to_string(&tool_calls).unwrap_or_default()),
                );
            }
    }

    // ── Lossless invariant guard: fallback message ───────────────────────
    let fallback_text = compute_fallback_text(
        hop_messages_for_lcm,
        output_text,
        is_final_hop,
        final_output_text,
    );

    if let Some(text) = fallback_text {
        let phase = if is_final_hop {
            Some(AssistantPhase::FinalAnswer)
        } else {
            Some(AssistantPhase::Commentary)
        };
        let mut msg = StoredMessage::new(
            lcm::MessageId::new(),
            conversation_id,
            lcm::MessageRole::Assistant,
            &text,
            lcm::estimate_tokens(&text),
            now_ms,
            0,
            turn_idx as u32,
        );
        msg.metadata.insert(
            "request_id".to_string(),
            Value::String(request_id.to_string()),
        );
        msg.metadata.insert(
            "response_id".to_string(),
            Value::String(response_id.to_string()),
        );
        msg.metadata.insert(
            "phase".to_string(),
            Value::String(phase.map(|p| p.as_str().to_string()).unwrap_or_else(|| {
                if is_final_hop {
                    "final_answer".to_string()
                } else {
                    "commentary".to_string()
                }
            })),
        );
        if let Some(ref thinking_text) = hop_thinking_text {
            msg.thinking = Some(thinking_text.clone());
        }
        batch_messages.push(msg);
    }

    if !batch_messages.is_empty()
        && let Err(e) = context.persist_messages(&batch_messages).await {
            log::warn!(
                "Failed to persist {} assistant messages: {}",
                batch_messages.len(),
                e
            );
        }
}

/// Persist tool execution results to LCM.
pub(crate) async fn persist_tool_results(
    context: &dyn AgentContext,
    conversation_id: &str,
    request_id: &str,
    turn_idx: usize,
    tool_results_items: &[Value],
    executed_tool_call_items: &[Value],
) {
    let now_ms = crate::conversation_store_utils::now_unix_ms();
    let mut batch_messages: Vec<StoredMessage> = Vec::new();

    let tool_name_by_call_id: std::collections::HashMap<String, String> = executed_tool_call_items
        .iter()
        .filter_map(|item| {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .map(String::from)?;
            let name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown")
                .to_string();
            Some((call_id, name))
        })
        .collect();

    for item in tool_results_items {
        if let Some(output_str) = item.get("output").and_then(|v| v.as_str()) {
            let call_id = item
                .get("call_id")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            let tool_name = tool_name_by_call_id
                .get(call_id)
                .map(String::as_str)
                .unwrap_or("unknown");

            let mut metadata = BTreeMap::new();
            metadata.insert(
                "message_type".to_string(),
                Value::String("function_call_output".to_string()),
            );
            metadata.insert("call_id".to_string(), Value::String(call_id.to_string()));
            metadata.insert(
                "tool_name".to_string(),
                Value::String(tool_name.to_string()),
            );
            metadata.insert(
                "request_id".to_string(),
                Value::String(request_id.to_string()),
            );

            let mut msg = StoredMessage::new(
                lcm::MessageId::new(),
                conversation_id,
                lcm::MessageRole::Tool,
                output_str,
                lcm::estimate_tokens(output_str),
                now_ms,
                0,
                turn_idx as u32,
            );
            msg.metadata = metadata;
            batch_messages.push(msg);
        }
    }

    if !batch_messages.is_empty()
        && let Err(e) = context.persist_messages(&batch_messages).await {
            log::warn!(
                "Failed to persist {} tool result messages: {}",
                batch_messages.len(),
                e
            );
        }
}

/// Compute a fallback text when no structured assistant messages were
/// extracted but the provider did produce output text.
fn compute_fallback_text(
    hop_messages_for_lcm: &[(String, Option<AssistantPhase>)],
    output_text: &str,
    is_final_hop: bool,
    final_output_text: &str,
) -> Option<String> {
    if hop_messages_for_lcm.is_empty() && !output_text.trim().is_empty() {
        Some(output_text.trim().to_string())
    } else if is_final_hop && !final_output_text.trim().is_empty() {
        let already_captured = hop_messages_for_lcm
            .iter()
            .any(|(t, _)| t.trim() == final_output_text.trim());
        if !already_captured {
            Some(final_output_text.trim().to_string())
        } else {
            None
        }
    } else {
        None
    }
}
