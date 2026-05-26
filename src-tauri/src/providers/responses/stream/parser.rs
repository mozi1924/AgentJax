use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::message_phase::AssistantPhase;
use crate::providers::types::{ProviderEventSink, ProviderStreamEvent};

pub(crate) struct ParserState {
    pub emitted_reasoning_started: bool,
    pub emitted_output_started: bool,
    pub active_tools_map: HashMap<String, String>,
    pub assistant_message_phase_by_item: HashMap<String, AssistantPhase>,
    pub completed_tool_calls: Vec<String>,
}

pub(crate) fn split_sse_event_block(buffer: &str) -> Option<(String, String)> {
    if let Some(pos) = buffer.find("\r\n\r\n") {
        let block = buffer[..pos].to_string();
        let rest = buffer[pos + 4..].to_string();
        return Some((block, rest));
    }
    if let Some(pos) = buffer.find("\n\n") {
        let block = buffer[..pos].to_string();
        let rest = buffer[pos + 2..].to_string();
        return Some((block, rest));
    }
    None
}

pub(crate) fn process_sse_event_block(
    block: &str,
    response_id: &mut String,
    output_text: &mut String,
    last_response_obj: &mut Option<Value>,
    state: &Mutex<ParserState>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let mut data_lines = Vec::new();

    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        return Ok(());
    }

    let payload = data_lines.join("\n");
    if payload == "[DONE]" || payload.trim().is_empty() {
        return Ok(());
    }

    handle_stream_event_json(
        &payload,
        response_id,
        output_text,
        last_response_obj,
        state,
        on_delta,
    )
}

pub(crate) fn collect_output_item_from_sse_event_block(
    block: &str,
    accumulated_output_items: &mut Vec<Value>,
) {
    let mut data_lines = Vec::new();
    for line in block.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(data) = line.strip_prefix("data:") {
            data_lines.push(data.trim_start().to_string());
        }
    }

    if data_lines.is_empty() {
        return;
    }

    let payload = data_lines.join("\n");
    if payload == "[DONE]" || payload.trim().is_empty() {
        return;
    }

    let Ok(value) = serde_json::from_str::<Value>(&payload) else {
        return;
    };

    if value.get("type").and_then(Value::as_str) == Some("response.output_item.done") {
        if let Some(item) = value.get("item") {
            accumulated_output_items.push(item.clone());
        }
    }
}

pub(crate) fn handle_stream_event_json(
    payload: &str,
    response_id: &mut String,
    output_text: &mut String,
    last_response_obj: &mut Option<Value>,
    state_mutex: &Mutex<ParserState>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let value: Value = serde_json::from_str(payload).map_err(|e| {
        format!(
            "Failed to parse streaming event: {e}. body={}",
            preview(payload)
        )
    })?;

    let event_type = value.get("type").and_then(Value::as_str).unwrap_or("");

    if event_type == "error" {
        if let Some(err) = value.get("error") {
            return Err(format!("Streaming error: {}", err));
        }
        return Err(format!("Streaming error: {}", value));
    }

    if let Some(error_obj) = value.get("error") {
        return Err(format!("Streaming error: {}", error_obj));
    }

    if response_id.is_empty() {
        if let Some(id) = value
            .get("response")
            .and_then(|r| r.get("id"))
            .and_then(Value::as_str)
        {
            *response_id = id.to_string();
        } else if let Some(id) = value.get("response_id").and_then(Value::as_str) {
            *response_id = id.to_string();
        } else if let Some(id) = value.get("id").and_then(Value::as_str) {
            *response_id = id.to_string();
        }
    }

    let mut state = state_mutex
        .lock()
        .map_err(|_| "Failed to lock ParserState".to_string())?;

    if !state.emitted_reasoning_started
        && matches!(
            event_type,
            "response.reasoning_text.delta"
                | "response.reasoning_text.done"
                | "response.reasoning_summary_text.delta"
                | "response.reasoning_summary_text.done"
                | "response.reasoning_summary_part.added"
                | "response.reasoning_summary_part.done"
        )
    {
        state.emitted_reasoning_started = true;
        on_delta(ProviderStreamEvent::ReasoningStarted)?;
    }

    if event_type == "response.output_text.delta" {
        if let Some(delta) = value.get("delta").and_then(Value::as_str) {
            if !state.emitted_output_started {
                state.emitted_output_started = true;
                on_delta(ProviderStreamEvent::OutputTextStarted)?;
            }
            let phase = value
                .get("item_id")
                .and_then(Value::as_str)
                .and_then(|item_id| state.assistant_message_phase_by_item.get(item_id).copied());
            output_text.push_str(delta);
            on_delta(ProviderStreamEvent::OutputTextDelta {
                delta: delta.to_string(),
                phase,
            })?;
        }
    }

    if let Some(done_text) = value.get("text").and_then(Value::as_str) {
        if event_type == "response.output_text.done" && output_text.is_empty() {
            if !state.emitted_output_started {
                state.emitted_output_started = true;
                on_delta(ProviderStreamEvent::OutputTextStarted)?;
            }
            output_text.push_str(done_text);
        }
    }

    if event_type == "response.output_item.added" {
        if let Some(item) = value.get("item") {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            if item_type == "message"
                && item.get("role").and_then(Value::as_str) == Some("assistant")
            {
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    if let Some(phase) = item
                        .get("phase")
                        .and_then(Value::as_str)
                        .and_then(AssistantPhase::from_api_value)
                    {
                        state
                            .assistant_message_phase_by_item
                            .insert(item_id.to_string(), phase);
                    }
                }
            }
            if item_type == "function_call" {
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                if !item_id.is_empty() && !call_id.is_empty() {
                    state.active_tools_map.insert(item_id.clone(), name.clone());
                    on_delta(ProviderStreamEvent::ToolCallStarted {
                        item_id,
                        call_id,
                        name,
                    })?;
                }
            }
        }
    }

    if event_type == "response.function_call_arguments.delta" {
        let item_id = value
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let call_id = value
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let delta = value
            .get("delta")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if !item_id.is_empty() && !call_id.is_empty() && !delta.is_empty() {
            on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                item_id,
                call_id,
                delta,
            })?;
        }
    }

    if event_type == "response.function_call_arguments.done" {
        let item_id = value
            .get("item_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let call_id = value
            .get("call_id")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let arguments = value
            .get("arguments")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();

        if !item_id.is_empty()
            && !call_id.is_empty()
            && !state.completed_tool_calls.contains(&call_id)
        {
            let name = state
                .active_tools_map
                .get(&item_id)
                .cloned()
                .unwrap_or_default();
            state.completed_tool_calls.push(call_id.clone());

            on_delta(ProviderStreamEvent::ToolCallCompleted {
                item_id,
                call_id,
                name,
                arguments,
            })?;
        }
    }

    if event_type == "response.output_item.done" {
        if let Some(item) = value.get("item") {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            if item_type == "message"
                && item.get("role").and_then(Value::as_str) == Some("assistant")
            {
                let phase = item
                    .get("phase")
                    .and_then(Value::as_str)
                    .and_then(AssistantPhase::from_api_value);
                if let Some(item_id) = item.get("id").and_then(Value::as_str) {
                    if let Some(phase) = phase {
                        state
                            .assistant_message_phase_by_item
                            .insert(item_id.to_string(), phase);
                    }
                }
                if let Some(phase) = phase {
                    let text = item
                        .get("content")
                        .and_then(Value::as_array)
                        .map(|content| {
                            content
                                .iter()
                                .filter_map(|part| part.get("text").and_then(Value::as_str))
                                .collect::<Vec<_>>()
                                .join("")
                        })
                        .unwrap_or_default();
                    if !text.trim().is_empty() {
                        on_delta(ProviderStreamEvent::AssistantMessageCompleted {
                            text,
                            phase,
                            response_id: response_id.clone(),
                        })?;
                    }
                }
            }
            if item_type == "function_call" {
                let item_id = item
                    .get("id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                if !item_id.is_empty()
                    && !call_id.is_empty()
                    && !state.completed_tool_calls.contains(&call_id)
                {
                    state.completed_tool_calls.push(call_id.clone());

                    on_delta(ProviderStreamEvent::ToolCallCompleted {
                        item_id,
                        call_id,
                        name,
                        arguments,
                    })?;
                }
            }
        }
    }

    if let Some(response_obj) = value.get("response").and_then(Value::as_object) {
        *last_response_obj = Some(Value::Object(response_obj.clone()));
    }

    Ok(())
}

pub(crate) fn extract_output_items(root: &Value) -> Vec<Value> {
    root.get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

#[derive(Debug, Clone)]
pub(crate) struct AssistantMessageChunk {
    pub text: String,
    pub phase: Option<AssistantPhase>,
}

pub(crate) fn extract_assistant_messages(root: &Value) -> Vec<AssistantMessageChunk> {
    let Some(output) = root.get("output").and_then(Value::as_array) else {
        return Vec::new();
    };

    output
        .iter()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
        .filter_map(|item| {
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            Some(AssistantMessageChunk {
                text,
                phase: item
                    .get("phase")
                    .and_then(Value::as_str)
                    .and_then(AssistantPhase::from_api_value),
            })
        })
        .collect()
}

pub(crate) fn extract_final_output_text(root: &Value) -> String {
    let assistant_messages = extract_assistant_messages(root);
    if let Some(final_text) = assistant_messages
        .iter()
        .rev()
        .find(|message| message.phase == Some(AssistantPhase::FinalAnswer))
        .map(|message| message.text.clone())
    {
        return final_text;
    }

    assistant_messages
        .last()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

pub(crate) fn extract_output_text(root: &Value) -> String {
    let final_output_text = extract_final_output_text(root);
    if !final_output_text.is_empty() {
        return final_output_text;
    }

    if let Some(choices) = root.get("choices").and_then(Value::as_array) {
        if let Some(first) = choices.first() {
            if let Some(message) = first.get("message") {
                if let Some(content) = message.get("content").and_then(Value::as_str) {
                    return content.to_string();
                }
                if let Some(content_items) = message.get("content").and_then(Value::as_array) {
                    let joined = content_items
                        .iter()
                        .filter_map(value_to_text)
                        .collect::<Vec<_>>()
                        .join("");
                    if !joined.is_empty() {
                        return joined;
                    }
                }
            }
            if let Some(text) = first.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
        }
    }

    if let Some(text) = root.get("text").and_then(Value::as_str) {
        return text.to_string();
    }

    String::new()
}

fn value_to_text(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    value
        .get("text")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}

fn preview(raw: &str) -> String {
    const MAX: usize = 400;
    if raw.len() <= MAX {
        raw.to_string()
    } else {
        format!("{}...[truncated]", &raw[..MAX])
    }
}

#[cfg(test)]
mod tests {
    use super::collect_output_item_from_sse_event_block;
    use serde_json::Value;

    #[test]
    fn collects_output_item_done_from_sse_block() {
        let block = r#"event: response.output_item.done
data: {"type":"response.output_item.done","item":{"type":"function_call","call_id":"call_1","name":"tool_a","arguments":"{}"}}

"#;

        let mut items: Vec<Value> = Vec::new();
        collect_output_item_from_sse_event_block(block, &mut items);
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].get("type").and_then(Value::as_str),
            Some("function_call")
        );
        assert_eq!(
            items[0].get("call_id").and_then(Value::as_str),
            Some("call_1")
        );
    }
}
