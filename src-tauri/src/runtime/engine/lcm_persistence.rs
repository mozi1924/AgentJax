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
#[allow(clippy::too_many_arguments)]
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
    // Reasoning segments are stored individually (not merged) so that
    // `context_to_provider_items` can reconstruct the original interleaving
    // of reasoning and function_calls.  Without this, post-tool-call
    // reasoning (e.g. after a failed tool) would appear before the tool
    // result in the reconstructed context.
    let reasoning_segments: Vec<String> = output_items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .filter_map(|item| item.get("text").and_then(Value::as_str))
        .filter(|text| !text.trim().is_empty())
        .map(|t| t.to_string())
        .collect();

    // Build an item-order manifest so context reconstruction can preserve
    // the original interleaving: reasoning → fc → reasoning → fc → text.
    let item_order: Vec<&str> = output_items
        .iter()
        .filter_map(|item| item.get("type").and_then(Value::as_str))
        .filter(|t| matches!(*t, "reasoning" | "function_call" | "message"))
        .collect();

    // Pre-tool-call reasoning goes on the first assistant message (legacy
    // compatibility).  Additional reasoning segments after the first
    // function_call are stored as separate thinking-only assistant messages
    // so they appear after the tool calls in reconstruction.
    let pre_tool_reasoning: Option<String> = {
        if reasoning_segments.is_empty() {
            None
        } else {
            // Find the index of the first function_call in item_order.
            let first_fc_idx = item_order
                .iter()
                .position(|t| *t == "function_call");
            match first_fc_idx {
                Some(idx) if idx > 0 => {
                    // Reasoning segments before the first function_call.
                    let count = item_order[..idx]
                        .iter()
                        .filter(|t| **t == "reasoning")
                        .count();
                    if count > 0 {
                        Some(reasoning_segments[..count].join("\n"))
                    } else {
                        None
                    }
                }
                Some(_) => None, // No reasoning before first fc
                None => {
                    // No function_calls at all — all reasoning goes on
                    // the first assistant message.
                    Some(reasoning_segments.join("\n"))
                }
            }
        }
    };

    // Attach pre-tool-call thinking to the first assistant message.
    if let Some(ref thinking_text) = pre_tool_reasoning
        && let Some(msg) = batch_messages
            .iter_mut()
            .find(|m| m.role == lcm::MessageRole::Assistant)
        {
            msg.thinking = Some(thinking_text.clone());
        }

    // ── Store item order for interleaved reconstruction ────────────────
    // The order manifest lets `context_to_provider_items` replay reasoning
    // and function_calls in their original sequence, preserving CoT
    // continuity when a tool fails and the model continues thinking.
    if !item_order.is_empty()
        && let Some(msg) = batch_messages
            .iter_mut()
            .find(|m| m.role == lcm::MessageRole::Assistant)
        {
            let order_json = serde_json::to_string(&item_order).unwrap_or_default();
            if !order_json.is_empty() && order_json != "[]" {
                msg.metadata.insert(
                    "output_item_order".to_string(),
                    Value::String(order_json),
                );
            }
        }

    // ── Post-tool-call reasoning as separate messages ──────────────────
    // Reasoning that appears after the first function_call is stored as
    // separate thinking-only assistant messages, placed AFTER the main
    // assistant message(s).  This preserves the natural interleaving:
    //   reasoning_1 → fc → [tool results] → reasoning_2 → fc → ...
    {
        let first_fc_idx = item_order
            .iter()
            .position(|t| *t == "function_call");
        if let Some(fc_idx) = first_fc_idx {
            let pre_fc_reasoning_count = item_order[..fc_idx]
                .iter()
                .filter(|t| **t == "reasoning")
                .count();
            // Reasoning segments after the pre-tool-call ones.
            for segment in reasoning_segments.iter().skip(pre_fc_reasoning_count) {
                let mut msg = StoredMessage::new(
                    lcm::MessageId::new(),
                    conversation_id,
                    lcm::MessageRole::Assistant,
                    "", // empty content — thinking-only message
                    0, // token count will be estimated by LCM
                    now_ms,
                    0,
                    turn_idx as u32,
                );
                msg.thinking = Some(segment.clone());
                msg.metadata.insert(
                    "request_id".to_string(),
                    Value::String(request_id.to_string()),
                );
                msg.metadata.insert(
                    "response_id".to_string(),
                    Value::String(response_id.to_string()),
                );
                msg.metadata.insert(
                    "thinking_only".to_string(),
                    Value::String("true".to_string()),
                );
                batch_messages.push(msg);
            }
        }
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
        if let Some(ref thinking_text) = pre_tool_reasoning {
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
