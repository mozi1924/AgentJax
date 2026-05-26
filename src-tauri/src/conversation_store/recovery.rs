use super::paths::{conversation_messages_path, conversation_metadata_path};
use super::{file_io::read_conversation_file, types::ConversationEntryLine};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};

fn parse_context_item_tool_arguments(raw: Option<&Value>) -> Value {
    let Some(value) = raw else {
        return json!({});
    };
    match value {
        Value::Object(_) => value.clone(),
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    }
}

pub fn build_recovery_developer_note(conversation_id: &str) -> Result<Option<Value>, String> {
    #[derive(Default)]
    struct RequestRecoveryState {
        last_at_unix_ms: i64,
        has_user_message: bool,
        has_assistant_message: bool,
        tool_calls: HashMap<String, (String, Value)>,
        tool_outputs: HashSet<String>,
    }

    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };
    data.entries.sort_by_key(|entry: &ConversationEntryLine| entry.created_at_unix_ms);

    let mut states: HashMap<String, RequestRecoveryState> = HashMap::new();

    for entry in data.entries {
        let Some(request_id) = entry
            .request_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
        else {
            continue;
        };

        let state = states.entry(request_id).or_default();
        state.last_at_unix_ms = state.last_at_unix_ms.max(entry.created_at_unix_ms);

        if entry.record_type == "message" {
            match entry.role.as_deref() {
                Some("user") => state.has_user_message = true,
                Some("assistant") => {
                    if entry
                        .text
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .is_some()
                    {
                        state.has_assistant_message = true;
                    }
                }
                _ => {}
            }
        }

        for item in &entry.context_items {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match item_type {
                "function_call" | "custom_tool_call" => {
                    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(name) = item.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    state
                        .tool_calls
                        .entry(call_id.to_string())
                        .or_insert_with(|| {
                            (
                                name.to_string(),
                                parse_context_item_tool_arguments(item.get("arguments")),
                            )
                        });
                }
                "function_call_output" | "custom_tool_call_output" => {
                    if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                        state.tool_outputs.insert(call_id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let mut selected: Option<(String, RequestRecoveryState)> = None;
    for (request_id, state) in states {
        if !state.has_user_message {
            continue;
        }

        let unresolved_calls: Vec<_> = state
            .tool_calls
            .keys()
            .filter(|call_id| !state.tool_outputs.contains(*call_id))
            .collect();
        let incomplete = !state.has_assistant_message || !unresolved_calls.is_empty();
        if !incomplete {
            continue;
        }

        let replace = selected
            .as_ref()
            .map(|(_, prev)| state.last_at_unix_ms > prev.last_at_unix_ms)
            .unwrap_or(true);
        if replace {
            selected = Some((request_id, state));
        }
    }

    let Some((request_id, state)) = selected else {
        return Ok(None);
    };

    let mut unresolved = Vec::new();
    for (call_id, (name, arguments)) in state.tool_calls {
        if state.tool_outputs.contains(&call_id) {
            continue;
        }
        unresolved.push(json!({
            "call_id": call_id,
            "tool": name,
            "arguments": arguments,
        }));
    }

    let payload = json!({
        "recovery_type": "unfinished_turn",
        "request_id": request_id,
        "assistant_message_missing": !state.has_assistant_message,
        "unresolved_tool_calls": unresolved,
    });
    let note = format!(
        "RECOVERY_CONTEXT {}\nPrevious request was interrupted. Continue from this state, avoid repeating completed steps, and resolve unresolved tool calls if still needed.",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );

    Ok(Some(json!({
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": note
        }]
    })))
}

