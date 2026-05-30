use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::providers::core::ProviderIdFactory;
use crate::providers::sse::sse_data_payload;
use crate::providers::types::{ProviderEventSink, ProviderStreamEvent, ProviderUsage};

#[derive(Debug, Clone, Default)]
pub(super) struct ChatToolCallAccumulator {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    started: bool,
    completed: bool,
}

/// Parse one Chat Completions SSE block into AgentJax provider stream events.
#[allow(clippy::too_many_arguments)]
pub(super) fn process_chat_completions_event(
    block: &str,
    response_id: &mut String,
    output_text: &mut String,
    usage: &mut Option<ProviderUsage>,
    emitted_output_started: &mut bool,
    id_factory: &mut ProviderIdFactory,
    tool_calls_by_index: &mut BTreeMap<usize, ChatToolCallAccumulator>,
    output_items: &mut Vec<Value>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let Some(payload) = sse_data_payload(block) else {
        return Ok(());
    };
    if payload == "[DONE]" || payload.trim().is_empty() {
        return Ok(());
    }

    let value: Value = serde_json::from_str(&payload).map_err(|err| {
        format!(
            "Failed to parse Chat Completions streaming event: {err}. body={}",
            preview(&payload)
        )
    })?;
    if let Some(error) = value.get("error") {
        return Err(format!("Chat Completions streaming error: {error}"));
    }
    if response_id.is_empty() {
        if let Some(id) = value.get("id").and_then(Value::as_str) {
            *response_id = id.to_string();
        }
    }
    if let Some(next_usage) = ProviderUsage::from_api_value(&value) {
        *usage = Some(next_usage);
    }

    let Some(choices) = value.get("choices").and_then(Value::as_array) else {
        return Ok(());
    };
    for choice in choices {
        if let Some(delta) = choice.get("delta") {
            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    if !*emitted_output_started {
                        *emitted_output_started = true;
                        on_delta(ProviderStreamEvent::OutputTextStarted)?;
                    }
                    output_text.push_str(content);
                    on_delta(ProviderStreamEvent::OutputTextDelta {
                        delta: content.to_string(),
                        phase: None,
                    })?;
                }
            }
            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                process_tool_call_deltas(tool_calls, id_factory, tool_calls_by_index, on_delta)?;
            }
        }

        if choice.get("finish_reason").and_then(Value::as_str) == Some("tool_calls") {
            finalize_pending_tool_calls(tool_calls_by_index, output_items, on_delta)?;
        }
    }

    Ok(())
}

fn process_tool_call_deltas(
    tool_calls: &[Value],
    id_factory: &mut ProviderIdFactory,
    tool_calls_by_index: &mut BTreeMap<usize, ChatToolCallAccumulator>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    for tool_call in tool_calls {
        let index = tool_call.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
        let function = tool_call.get("function").unwrap_or(&Value::Null);
        let name_delta = function.get("name").and_then(Value::as_str).unwrap_or("");
        let arguments_delta = function
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("");

        let entry = tool_calls_by_index.entry(index).or_default();
        if entry.call_id.is_empty() {
            entry.call_id = tool_call
                .get("id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| id_factory.next_call_id(name_delta));
        }
        if entry.item_id.is_empty() {
            entry.item_id = id_factory.next_item_id(name_delta);
        }
        if !name_delta.is_empty() {
            entry.name.push_str(name_delta);
        }

        if !entry.started && !entry.name.is_empty() {
            entry.started = true;
            on_delta(ProviderStreamEvent::ToolCallStarted {
                item_id: entry.item_id.clone(),
                call_id: entry.call_id.clone(),
                name: entry.name.clone(),
                presentation: None,
            })?;
        }

        if !arguments_delta.is_empty() {
            entry.arguments.push_str(arguments_delta);
            on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                item_id: entry.item_id.clone(),
                call_id: entry.call_id.clone(),
                delta: arguments_delta.to_string(),
            })?;
        }
    }

    Ok(())
}

pub(super) fn finalize_pending_tool_calls(
    tool_calls_by_index: &mut BTreeMap<usize, ChatToolCallAccumulator>,
    output_items: &mut Vec<Value>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    for call in tool_calls_by_index.values_mut() {
        if call.completed || call.call_id.is_empty() || call.name.is_empty() {
            continue;
        }
        call.completed = true;
        on_delta(ProviderStreamEvent::ToolCallCompleted {
            item_id: call.item_id.clone(),
            call_id: call.call_id.clone(),
            name: call.name.clone(),
            arguments: call.arguments.clone(),
            presentation: None,
        })?;
        output_items.push(json!({
            "type": "function_call",
            "id": call.item_id,
            "call_id": call.call_id,
            "name": call.name,
            "arguments": call.arguments
        }));
    }

    Ok(())
}

fn preview(raw: &str) -> String {
    const MAX: usize = 400;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...[truncated]", &raw[..MAX])
    }
}
