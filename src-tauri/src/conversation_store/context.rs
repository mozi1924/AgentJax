use super::file_io::read_conversation_file;
use super::items::{build_assistant_output_items, build_user_input_items};
use super::paths::{conversation_messages_path, conversation_metadata_path};
use super::types::ConversationContext;
use super::MAX_CONTEXT_ITEMS_PER_REQUEST;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn load_context_for_request(conversation_id: &str) -> Result<ConversationContext, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(ConversationContext::default());
    };

    data.entries.sort_by_key(|entry| entry.created_at_unix_ms);

    let mut context = ConversationContext::default();
    for entry in data.entries {
        if entry.record_type != "message" {
            if !entry.context_items.is_empty() {
                context.input_items.extend(entry.context_items);
            }
            continue;
        }

        if !entry.context_items.is_empty() {
            context.input_items.extend(entry.context_items);
            continue;
        }

        match entry.role.as_deref() {
            Some("user") => {
                if let Some(text) = entry.text.as_deref() {
                    context.input_items.extend(build_user_input_items(text));
                }
            }
            Some("assistant") => {
                if let Some(text) = entry.text.as_deref() {
                    context
                        .input_items
                        .extend(build_assistant_output_items(text));
                }
            }
            _ => {}
        }
    }

    context.input_items = sanitize_tool_call_pairs(context.input_items);
    context.input_items = truncate_context_items_preserving_tool_pairs(
        context.input_items,
        MAX_CONTEXT_ITEMS_PER_REQUEST,
    );
    Ok(context)
}

pub(super) fn sanitize_tool_call_pairs(items: Vec<Value>) -> Vec<Value> {
    let mut function_call_ids = HashSet::new();
    let mut function_call_output_ids = HashSet::new();
    let mut custom_call_ids = HashSet::new();
    let mut custom_call_output_ids = HashSet::new();

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
            Some("custom_tool_call") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    custom_call_ids.insert(call_id.to_string());
                }
            }
            Some("custom_tool_call_output") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    custom_call_output_ids.insert(call_id.to_string());
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
            Some("custom_tool_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| custom_call_output_ids.contains(call_id))
                .unwrap_or(false),
            Some("custom_tool_call_output") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| custom_call_ids.contains(call_id))
                .unwrap_or(false),
            _ => true,
        })
        .collect()
}

fn truncate_context_items_preserving_tool_pairs(items: Vec<Value>, max_items: usize) -> Vec<Value> {
    if items.len() <= max_items {
        return items;
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    enum ToolKind {
        Function,
        Custom,
    }

    let mut groups: HashMap<(ToolKind, String), Vec<usize>> = HashMap::new();
    for (idx, item) in items.iter().enumerate() {
        let kind = match item.get("type").and_then(Value::as_str) {
            Some("function_call") | Some("function_call_output") => Some(ToolKind::Function),
            Some("custom_tool_call") | Some("custom_tool_call_output") => Some(ToolKind::Custom),
            _ => None,
        };
        let Some(kind) = kind else {
            continue;
        };
        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        groups
            .entry((kind, call_id.to_string()))
            .or_default()
            .push(idx);
    }

    let mut selected = vec![false; items.len()];
    let mut kept = 0usize;

    for idx in (0..items.len()).rev() {
        if selected[idx] {
            continue;
        }

        let group_indices: Vec<usize> = match items[idx].get("type").and_then(Value::as_str) {
            Some("function_call") | Some("function_call_output") => {
                match items[idx].get("call_id").and_then(Value::as_str) {
                    Some(call_id) => groups
                        .get(&(ToolKind::Function, call_id.to_string()))
                        .cloned()
                        .unwrap_or_else(|| vec![idx]),
                    None => vec![idx],
                }
            }
            Some("custom_tool_call") | Some("custom_tool_call_output") => {
                match items[idx].get("call_id").and_then(Value::as_str) {
                    Some(call_id) => groups
                        .get(&(ToolKind::Custom, call_id.to_string()))
                        .cloned()
                        .unwrap_or_else(|| vec![idx]),
                    None => vec![idx],
                }
            }
            _ => vec![idx],
        };

        let new_items = group_indices
            .iter()
            .filter(|&&group_idx| !selected[group_idx])
            .count();
        if kept + new_items > max_items {
            continue;
        }

        for group_idx in group_indices {
            if !selected[group_idx] {
                selected[group_idx] = true;
                kept += 1;
            }
        }

        if kept >= max_items {
            break;
        }
    }

    items
        .into_iter()
        .enumerate()
        .filter_map(|(idx, item)| if selected[idx] { Some(item) } else { None })
        .collect()
}
