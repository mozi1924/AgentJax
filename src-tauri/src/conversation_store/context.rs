use super::file_io::read_conversation_file;
use super::paths::{conversation_messages_path, conversation_metadata_path};
use super::types::{ConversationContext, ConversationLine};
use crate::message_phase::AssistantPhase;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

const MAX_CONTEXT_ITEMS_PER_REQUEST: usize = 200;

/// Load conversation lines and flatten them into Responses-API-compatible
/// input items.  Each `user` / `assistant` / `tool` line is converted to
/// the standard `role`/`content` or `type`/`call_id`/`output` item format
/// that the OpenAI Responses endpoint expects.

pub fn load_context_for_request(conversation_id: &str) -> Result<ConversationContext, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(ConversationContext::default());
    };

    let mut input_items: Vec<Value> = Vec::new();

    for line in &data.lines {
        match line {
            ConversationLine::User(u) => {
                input_items.push(json!({
                    "role": "user",
                    "content": [{"type": "input_text", "text": u.text}]
                }));
            }
            ConversationLine::Assistant(a) => {
                if a.phase != AssistantPhase::FinalAnswer || a.text.trim().is_empty() {
                    continue;
                }
                input_items.push(json!({
                    "type": "message",
                    "role": "assistant",
                    "status": "completed",
                    "content": [{"type": "output_text", "text": a.text, "annotations": []}]
                }));
            }
            ConversationLine::Tool(t) => {
                // function_call
                let call_id = &t.call_id;
                input_items.push(json!({
                    "type": "function_call",
                    "call_id": call_id,
                    "name": t.name,
                    "arguments": t.args,
                }));
                // function_call_output (if available)
                if let Some(output) = &t.output {
                    input_items.push(json!({
                        "type": "function_call_output",
                        "call_id": call_id,
                        "output": serde_json::to_string(output).unwrap_or_else(|_| "{}".to_string()),
                    }));
                }
            }
        }
    }

    // Deduplicate and pair tool calls
    input_items = sanitize_tool_call_pairs(input_items);
    // Truncate if too large, preserving tool-call pairs
    input_items =
        truncate_context_items_preserving_tool_pairs(input_items, MAX_CONTEXT_ITEMS_PER_REQUEST);

    Ok(ConversationContext { input_items })
}

// ── Sanitisation helpers ──────────────────────────────────────────────────

pub(super) fn sanitize_tool_call_pairs(items: Vec<Value>) -> Vec<Value> {
    let mut function_call_ids = HashSet::new();
    let mut function_call_output_ids = HashSet::new();

    for item in &items {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    function_call_ids.insert(call_id.to_string());
                }
            }
            Some("function_call_output") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    function_call_output_ids.insert(call_id.to_string());
                }
            }
            _ => {}
        }
    }

    items
        .into_iter()
        .filter(|item| match item.get("type").and_then(Value::as_str) {
            Some("function_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| function_call_output_ids.contains(call_id))
                .unwrap_or(false),
            Some("function_call_output") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| function_call_ids.contains(call_id))
                .unwrap_or(false),
            _ => true,
        })
        .collect()
}

fn truncate_context_items_preserving_tool_pairs(items: Vec<Value>, max: usize) -> Vec<Value> {
    if items.len() <= max {
        return items;
    }

    // Walk backwards from max to find a safe cut point (don't split
    // function_call/function_call_output pairs).
    let skip = items.len() - max;
    let mut paired: HashMap<String, bool> = HashMap::new();

    for item in items.iter().skip(skip) {
        if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
            let kind = item.get("type").and_then(Value::as_str).unwrap_or("");
            if kind == "function_call" {
                paired
                    .entry(call_id.to_string())
                    .and_modify(|v| *v = true)
                    .or_insert(true);
            } else if kind == "function_call_output" {
                paired.entry(call_id.to_string()).or_insert(false);
            }
        }
    }

    let mut safe_skip = skip;
    for item in items.iter().take(skip).rev() {
        if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
            if paired.get(call_id).copied().unwrap_or(false) {
                safe_skip -= 1;
            }
        } else {
            // non-tool items are safe to drop
        }
    }

    items.into_iter().skip(safe_skip).collect()
}
