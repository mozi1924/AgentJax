use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::providers::core::ProviderIdFactory;
use crate::providers::sse::sse_data_payload;
use crate::providers::types::{ProviderEventSink, ProviderStreamEvent, ProviderUsage};

#[derive(Debug, Clone, Default)]
pub(super) struct AnthropicToolBlock {
    item_id: String,
    call_id: String,
    name: String,
    arguments_json: String,
    completed: bool,
}

/// Parse one Anthropic SSE event block and emit normalized provider events.
#[allow(clippy::too_many_arguments)]
pub(super) fn process_anthropic_event(
    block: &str,
    response_id: &mut String,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    emitted_output_started: &mut bool,
    id_factory: &mut ProviderIdFactory,
    tool_blocks_by_index: &mut BTreeMap<usize, AnthropicToolBlock>,
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
            "Failed to parse Anthropic streaming event: {err}. body={}",
            preview(&payload)
        )
    })?;
    if let Some(error) = value.get("error") {
        return Err(format!("Anthropic streaming error: {error}"));
    }

    match value.get("type").and_then(Value::as_str).unwrap_or("") {
        "message_start" => {
            if response_id.is_empty() {
                if let Some(id) = value
                    .get("message")
                    .and_then(|message| message.get("id"))
                    .and_then(Value::as_str)
                {
                    *response_id = id.to_string();
                }
            }
            merge_usage(usage, anthropic_usage_from_value(&value, usage.as_ref()));
        }
        "content_block_start" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(block) = value.get("content_block") {
                if block.get("type").and_then(Value::as_str) == Some("tool_use") {
                    start_tool_block(index, block, id_factory, tool_blocks_by_index, on_delta)?;
                }
            }
        }
        "content_block_delta" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            if let Some(delta) = value.get("delta") {
                process_content_delta(
                    index,
                    delta,
                    output_text,
                    emitted_output_started,
                    tool_blocks_by_index,
                    on_delta,
                )?;
            }
        }
        "content_block_stop" => {
            let index = value.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
            finalize_tool_block(index, tool_blocks_by_index, output_items, on_delta)?;
        }
        "message_delta" => {
            merge_usage(usage, anthropic_usage_from_value(&value, usage.as_ref()));
        }
        _ => {}
    }

    Ok(())
}

fn start_tool_block(
    index: usize,
    block: &Value,
    id_factory: &mut ProviderIdFactory,
    tool_blocks_by_index: &mut BTreeMap<usize, AnthropicToolBlock>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let name = block
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.trim().is_empty() {
        return Ok(());
    }
    let call_id = block
        .get("id")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| id_factory.next_call_id(&name));
    let item_id = id_factory.next_item_id(&name);
    let initial_arguments = block
        .get("input")
        .filter(|input| {
            input.is_object()
                && input
                    .as_object()
                    .map(|obj| !obj.is_empty())
                    .unwrap_or(false)
        })
        .map(|input| serde_json::to_string(input).unwrap_or_else(|_| "{}".to_string()))
        .unwrap_or_default();
    let tool_block = AnthropicToolBlock {
        item_id: item_id.clone(),
        call_id: call_id.clone(),
        name: name.clone(),
        arguments_json: initial_arguments,
        completed: false,
    };
    tool_blocks_by_index.insert(index, tool_block);
    on_delta(ProviderStreamEvent::ToolCallStarted {
        item_id,
        call_id,
        name,
        presentation: None,
    })?;
    Ok(())
}

fn process_content_delta(
    index: usize,
    delta: &Value,
    output_text: &mut String,
    emitted_output_started: &mut bool,
    tool_blocks_by_index: &mut BTreeMap<usize, AnthropicToolBlock>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    match delta.get("type").and_then(Value::as_str).unwrap_or("") {
        "text_delta" => {
            let text = delta.get("text").and_then(Value::as_str).unwrap_or("");
            if !text.is_empty() {
                if !*emitted_output_started {
                    *emitted_output_started = true;
                    on_delta(ProviderStreamEvent::OutputTextStarted)?;
                }
                output_text.push_str(text);
                on_delta(ProviderStreamEvent::OutputTextDelta {
                    delta: text.to_string(),
                    phase: None,
                })?;
            }
        }
        "input_json_delta" => {
            let partial_json = delta
                .get("partial_json")
                .and_then(Value::as_str)
                .unwrap_or("");
            if partial_json.is_empty() {
                return Ok(());
            }
            if let Some(tool_block) = tool_blocks_by_index.get_mut(&index) {
                tool_block.arguments_json.push_str(partial_json);
                on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                    item_id: tool_block.item_id.clone(),
                    call_id: tool_block.call_id.clone(),
                    delta: partial_json.to_string(),
                })?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn finalize_tool_block(
    index: usize,
    tool_blocks_by_index: &mut BTreeMap<usize, AnthropicToolBlock>,
    output_items: &mut Vec<Value>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let Some(tool_block) = tool_blocks_by_index.get_mut(&index) else {
        return Ok(());
    };
    if tool_block.completed {
        return Ok(());
    }
    tool_block.completed = true;
    let arguments = if tool_block.arguments_json.trim().is_empty() {
        "{}".to_string()
    } else {
        tool_block.arguments_json.clone()
    };
    on_delta(ProviderStreamEvent::ToolCallCompleted {
        item_id: tool_block.item_id.clone(),
        call_id: tool_block.call_id.clone(),
        name: tool_block.name.clone(),
        arguments: arguments.clone(),
        presentation: None,
    })?;
    output_items.push(json!({
        "type": "function_call",
        "id": tool_block.item_id,
        "call_id": tool_block.call_id,
        "name": tool_block.name,
        "arguments": arguments
    }));
    Ok(())
}

pub(super) fn finalize_all_tool_blocks(
    tool_blocks_by_index: &mut BTreeMap<usize, AnthropicToolBlock>,
    output_items: &mut Vec<Value>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let indexes = tool_blocks_by_index.keys().copied().collect::<Vec<_>>();
    for index in indexes {
        finalize_tool_block(index, tool_blocks_by_index, output_items, on_delta)?;
    }
    Ok(())
}

fn anthropic_usage_from_value(
    value: &Value,
    previous: Option<&ProviderUsage>,
) -> Option<ProviderUsage> {
    let usage = value
        .get("message")
        .and_then(|message| message.get("usage"))
        .or_else(|| value.get("usage"))?;
    let input_tokens = usage_usize(usage, "input_tokens")
        .saturating_add(usage_usize(usage, "cache_creation_input_tokens"))
        .saturating_add(usage_usize(usage, "cache_read_input_tokens"));
    let output_tokens = usage_usize(usage, "output_tokens");

    let prompt_tokens = if input_tokens > 0 {
        input_tokens
    } else {
        previous.map(|usage| usage.prompt_tokens).unwrap_or(0)
    };
    let completion_tokens = if output_tokens > 0 {
        output_tokens
    } else {
        previous.map(|usage| usage.completion_tokens).unwrap_or(0)
    };
    let total_tokens = prompt_tokens.saturating_add(completion_tokens);

    (prompt_tokens > 0 || completion_tokens > 0).then_some(ProviderUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

fn merge_usage(current: &mut Option<ProviderUsage>, next: Option<ProviderUsage>) {
    if let Some(next) = next {
        *current = Some(next);
    }
}

fn usage_usize(usage: &Value, key: &str) -> usize {
    usage.get(key).and_then(Value::as_u64).unwrap_or(0) as usize
}

fn preview(raw: &str) -> String {
    const MAX: usize = 400;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...[truncated]", &raw[..MAX])
    }
}
