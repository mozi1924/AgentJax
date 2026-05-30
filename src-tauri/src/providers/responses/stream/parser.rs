//! Parser for OpenAI Responses-compatible SSE events.
//!
//! This module is intentionally not a generic stream parser. Native Gemini,
//! Anthropic, or Chat Completions adapters should parse their own upstream
//! events and emit AgentJax's normalized `ProviderStreamEvent` values instead.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;

use crate::message_phase::AssistantPhase;
use crate::providers::sse::sse_data_payload;
use crate::providers::types::{ProviderEventSink, ProviderStreamEvent, ProviderUsage};

mod output;

pub(crate) use output::{extract_output_items, extract_output_text};

pub(crate) struct ParserState {
    pub emitted_reasoning_started: bool,
    pub emitted_output_started: bool,
    pub active_tools_map: HashMap<String, String>,
    pub assistant_message_phase_by_item: HashMap<String, AssistantPhase>,
    pub completed_tool_calls: Vec<String>,
    pub detected_usage: Option<ProviderUsage>,
}

pub(crate) fn process_sse_event_block(
    block: &str,
    response_id: &mut String,
    output_text: &mut String,
    last_response_obj: &mut Option<Value>,
    state: &Mutex<ParserState>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let Some(payload) = sse_data_payload(block) else {
        return Ok(());
    };
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
    let Some(payload) = sse_data_payload(block) else {
        return;
    };
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

    if let Some(usage) = ProviderUsage::from_api_value(&value) {
        let mut state = state_mutex
            .lock()
            .map_err(|_| "Failed to lock ParserState".to_string())?;
        state.detected_usage = Some(usage);
        drop(state);
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
                        presentation: None,
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
                presentation: None,
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
                        presentation: None,
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
    use super::{ParserState, collect_output_item_from_sse_event_block, handle_stream_event_json};
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::providers::types::ProviderStreamEvent;
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

    #[test]
    fn captures_provider_usage_from_response_done_event() {
        let state = Mutex::new(ParserState {
            emitted_reasoning_started: false,
            emitted_output_started: false,
            active_tools_map: HashMap::new(),
            assistant_message_phase_by_item: HashMap::new(),
            completed_tool_calls: Vec::new(),
            detected_usage: None,
        });
        let mut response_id = String::new();
        let mut output_text = String::new();
        let mut last_response_obj = None;
        let mut events: Vec<ProviderStreamEvent> = Vec::new();
        let payload = r#"{
            "type":"response.done",
            "response":{
                "id":"resp_1",
                "usage":{
                    "input_tokens":11,
                    "output_tokens":7,
                    "total_tokens":18
                }
            }
        }"#;

        handle_stream_event_json(
            payload,
            &mut response_id,
            &mut output_text,
            &mut last_response_obj,
            &state,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();

        let usage = state.lock().unwrap().detected_usage.clone().unwrap();
        assert_eq!(usage.prompt_tokens, 11);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 18);
        assert!(events.is_empty());
    }
}
