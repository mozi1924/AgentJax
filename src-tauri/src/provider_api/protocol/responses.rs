//! OpenAI Responses API protocol implementation.

use crate::config::{AppConfig, ProviderConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::capabilities::ProviderCapabilities;
use crate::provider_api::core::ProviderIdFactory;
use crate::provider_api::network::{apply_headers_to_reqwest, split_sse_event_block};
use crate::provider_api::protocol::{build_client, send_and_check};
use crate::provider_api::types::*;
use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::watch;

/// Stream a response using the OpenAI Responses API.
pub async fn stream_response<F>(
    config: &AppConfig,
    provider_key: &str,
    provider_config: &ProviderConfig,
    model_id: &str,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    mut on_delta: F,
) -> AgentJaxResult<ResponseStreamResult>
where
    F: FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send,
{
    let timeout_seconds = provider_config.resolved_timeout_seconds(config.request_timeout_seconds);
    let client = build_client(timeout_seconds)?;

    let base_url = provider_config.api_endpoint().trim_end_matches('/').to_string();
    let url = format!("{base_url}/responses");

    let body = build_response_payload(model_id, req);

    let credential = provider_config.resolved_credential();
    let mut builder = client.post(&url).json(&body);
    if let Some(ref credential) = credential {
        builder = builder.header("Authorization", format!("Bearer {credential}"));
    }
    let headers = provider_config.resolved_http_headers();
    builder = apply_headers_to_reqwest(builder, &headers)?;
    let response = send_and_check(builder, provider_key).await?;

    // ── Parse SSE stream inline ──
    let mut state = ResponsesStreamState::new();
    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items: Vec<Value> = Vec::new();
    let mut usage: Option<ProviderUsage> = None;
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut stream_done = false;

    while !stream_done {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() { break; }
            }
            next_chunk = stream.next() => {
                let Some(next_chunk) = next_chunk else { break; };
                let bytes = next_chunk
                    .map_err(|err| AgentJaxError::network(format!("Failed to read stream: {err}")))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    if process_responses_event(
                        &event_block, &mut state,
                        &mut response_id, &mut output_text, &mut output_items, &mut usage,
                        &mut on_delta,
                    )? {
                        stream_done = true;
                        break;
                    }
                }
            }
        }
    }

    if !stream_done && !buffer.trim().is_empty() {
        let _ = process_responses_event(
            &buffer, &mut state,
            &mut response_id, &mut output_text, &mut output_items, &mut usage,
            &mut on_delta,
        )?;
    }

    // Flush any remaining reasoning that wasn't terminated by an event.
    if state.reasoning_started && !state.reasoning_buffer.is_empty() {
        state.reasoning_started = false;
        let _ = on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None });
        output_items.push(json!({
            "type": "reasoning",
            "text": state.reasoning_buffer.clone(),
        }));
        state.reasoning_buffer.clear();
    }

    let final_response_id = if response_id.is_empty() {
        ProviderIdFactory::new(provider_key).response_id().to_string()
    } else {
        response_id
    };

    let usage_hops = usage.clone()
        .map(|u| ProviderUsageRecord {
            response_id: final_response_id.clone(),
            usage: u,
        })
        .into_iter()
        .collect();

    Ok(ResponseStreamResult {
        response_id: final_response_id,
        output_text,
        output_items,
        usage,
        usage_hops,
        provider_key: provider_key.to_string(),
        model_profile: format!("{provider_key}/{model_id}"),
        model_id: model_id.to_string(),
        capabilities: ProviderCapabilities::openai_responses(),
        reasoning_text: None,
        reasoning_tokens: None,
    })
}

// ── Request Building ─────────────────────────────────────────────────────────

fn build_response_payload(model_id: &str, req: &ResponseStreamRequest) -> Value {
    let mut payload = json!({
        "model": model_id,
        "input": normalize_input_items(&req.input_items),
        "store": false,
        "stream": true,
    });

    let instructions = req.instructions_override.as_deref()
        .filter(|s| !s.trim().is_empty()).map(String::from);
    if let Some(ref instructions) = instructions {
        payload["instructions"] = json!(instructions);
    }
    if let Some(ref effort) = req.reasoning_effort {
        let trimmed = effort.trim().to_lowercase();
        if !trimmed.is_empty() {
            payload["reasoning"] = json!({ "effort": trimmed });
        }
    }
    if let Some(ref tools) = req.tools && !tools.is_empty() { payload["tools"] = Value::Array(tools.clone()); }
    if let Some(ref tool_choice) = req.tool_choice { payload["tool_choice"] = tool_choice.clone(); }
    if let Some(ref text) = req.text { payload["text"] = text.clone(); }
    if let Some(ref include) = req.include && !include.is_empty() { payload["include"] = Value::Array(include.iter().map(|s| json!(s)).collect()); }
    payload
}

fn normalize_input_items(items: &[Value]) -> Value {
    let normalized: Vec<Value> = items.iter().map(|item| {
        let mut cloned = item.clone();
        if let Some(obj) = cloned.as_object_mut() {
            let item_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
            if item_type != "function_call" && item_type != "function_call_output" {
                obj.remove("id");
            }
            let role = obj.get("role").and_then(Value::as_str).unwrap_or("").to_string();
            if let Some(content) = obj.get_mut("content").and_then(|v| v.as_array_mut()) {
                for part in content.iter_mut() {
                    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("").to_string();
                    if matches!(part_type.as_str(), "text" | "input_text" | "output_text") {
                        part["type"] = json!(if role == "assistant" { "output_text" } else { "input_text" });
                    }
                }
            }
        }
        cloned
    }).collect();
    Value::Array(normalized)
}

// ── SSE Event Processing ─────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn process_responses_event(
    event_block: &str,
    state: &mut ResponsesStreamState,
    response_id: &mut String,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    on_delta: &mut dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()>,
) -> AgentJaxResult<bool> {
    let data = extract_sse_data(event_block);
    if data.is_empty() || data == "[DONE]" { return Ok(data == "[DONE]"); }

    let value: Value = serde_json::from_str(&data)
        .map_err(|_| AgentJaxError::internal("Failed to parse SSE JSON"))?;
    if let Some(err) = value.get("error") {
        return Err(AgentJaxError::internal(format!("Responses API error: {err}")));
    }

    if let Some(id) = value.pointer("/response/id")
        .or_else(|| value.get("response_id"))
        .or_else(|| value.get("id"))
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
    { *response_id = id.to_string(); }

    if let Some(u) = parse_responses_usage(&value) { *usage = Some(u); }

    let type_str = value.get("type").and_then(Value::as_str).unwrap_or("");
    let done = type_str == "response.completed" || type_str == "response.done";

    match type_str {
        "response.output_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                if !state.emitted_output_started {
                    state.emitted_output_started = true;
                    on_delta(ProviderStreamEvent::OutputTextStarted)?;
                }
                output_text.push_str(delta);
                on_delta(ProviderStreamEvent::OutputTextDelta { delta: delta.to_string(), phase: None })?;
            }
        }
        "response.output_item.added" => {
            if let Some(item) = value.get("item")
                && item.get("type").and_then(Value::as_str) == Some("function_call")
            {
                on_delta(ProviderStreamEvent::ToolCallStarted {
                    item_id: item.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                    call_id: item.get("call_id").and_then(Value::as_str).unwrap_or("").to_string(),
                    name: item.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    presentation: None,
                })?;
            }
        }
        "response.function_call_arguments.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                    item_id: value.get("item_id").and_then(Value::as_str).unwrap_or("").to_string(),
                    call_id: value.get("call_id").and_then(Value::as_str).unwrap_or("").to_string(),
                    delta: delta.to_string(),
                })?;
            }
        }
        "response.function_call_arguments.done" => {
            if let Some(call_id) = value.get("call_id").and_then(Value::as_str) {
                state.completed_tool_calls.push(call_id.to_string());
                on_delta(ProviderStreamEvent::ToolCallCompleted {
                    item_id: value.get("item_id").and_then(Value::as_str).unwrap_or("").to_string(),
                    call_id: call_id.to_string(),
                    name: value.get("name").and_then(Value::as_str).unwrap_or("").to_string(),
                    arguments: value.get("arguments").and_then(Value::as_str).unwrap_or("{}").to_string(),
                    presentation: None,
                })?;
            }
        }
        "response.output_item.done" => {
            if let Some(item) = value.get("item")
                && item.get("type").and_then(Value::as_str) == Some("message")
            {
                let text = item.get("content")
                    .and_then(Value::as_array)
                    .map(|content| content.iter().filter_map(|part| part.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(""))
                    .unwrap_or_default();
                if !text.trim().is_empty() {
                    on_delta(ProviderStreamEvent::AssistantMessageCompleted {
                        text, phase: None, response_id: response_id.clone(),
                    })?;
                }
            }
        }
        // ── Reasoning / thinking events ──────────────────────────
        "response.reasoning.summary_part.added"
            if !state.reasoning_started =>
        {
            state.reasoning_started = true;
            on_delta(ProviderStreamEvent::ReasoningStarted)?;
        }
        "response.reasoning.summary_text.delta" => {
            if let Some(delta) = value.get("delta").and_then(Value::as_str)
                && !delta.is_empty()
            {
                if !state.reasoning_started {
                    state.reasoning_started = true;
                    on_delta(ProviderStreamEvent::ReasoningStarted)?;
                }
                state.reasoning_buffer.push_str(delta);
                on_delta(ProviderStreamEvent::ReasoningDelta { delta: delta.to_string() })?;
            }
        }
        "response.reasoning.summary_part.done"
            if state.reasoning_started && !state.reasoning_buffer.is_empty() =>
        {
            state.reasoning_started = false;
            let total_tokens = value.get("total_tokens")
                .and_then(|v| v.as_u64()).map(|v| v as usize);
            on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens })?;
            output_items.push(json!({
                "type": "reasoning",
                "text": state.reasoning_buffer.clone(),
            }));
            state.reasoning_buffer.clear();
        }
        "response.completed" | "response.done"
            if state.reasoning_started && !state.reasoning_buffer.is_empty() =>
        {
            state.reasoning_started = false;
            on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None })?;
            output_items.push(json!({
                "type": "reasoning",
                "text": state.reasoning_buffer.clone(),
            }));
            state.reasoning_buffer.clear();
        }
        _ => {}
    }
    Ok(done)
}

fn extract_sse_data(event_block: &str) -> String {
    let trimmed = event_block.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') { return trimmed.to_string(); }
    trimmed.lines()
        .filter(|line| line.starts_with("data:"))
        .map(|line| line[5..].trim_start())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_responses_usage(value: &Value) -> Option<ProviderUsage> {
    let usage_value = value.pointer("/response/usage").or_else(|| value.get("usage")).unwrap_or(value);
    let usage: ProviderUsage = serde_json::from_value(usage_value.clone()).ok()?;
    (usage.prompt_tokens > 0 || usage.completion_tokens > 0 || usage.total_tokens > 0).then_some(usage)
}

struct ResponsesStreamState {
    emitted_output_started: bool,
    reasoning_started: bool,
    reasoning_buffer: String,
    completed_tool_calls: Vec<String>,
}

impl ResponsesStreamState {
    fn new() -> Self { Self { emitted_output_started: false, reasoning_started: false, reasoning_buffer: String::new(), completed_tool_calls: Vec::new() } }
}
