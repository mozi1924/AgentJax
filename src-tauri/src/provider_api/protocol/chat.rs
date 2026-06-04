//! OpenAI Chat Completions API protocol implementation.

use crate::config::{AppConfig, ProviderConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::capabilities::ProviderCapabilities;
use crate::provider_api::core::ProviderIdFactory;
use crate::provider_api::network::{apply_headers_to_reqwest, split_sse_event_block};
use crate::provider_api::protocol::{build_client, send_and_check};
use crate::provider_api::types::*;
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::watch;

/// Stream a response using the OpenAI Chat Completions API.
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
    let url = format!("{base_url}/chat/completions");

    let body = build_chat_payload(model_id, req);

    let credential = provider_config.resolved_credential();
    let mut builder = client.post(&url).json(&body);
    if let Some(ref credential) = credential {
        builder = builder.header("Authorization", format!("Bearer {credential}"));
    }
    let headers = provider_config.resolved_http_headers();
    builder = apply_headers_to_reqwest(builder, &headers)?;
    let response = send_and_check(builder, provider_key).await?;

    // ── Parse SSE stream inline ──
    let mut state = ChatStreamState::new();
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
                    if process_chat_event(
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
        let _ = process_chat_event(
            &buffer, &mut state,
            &mut response_id, &mut output_text, &mut output_items, &mut usage,
            &mut on_delta,
        )?;
    }

    let final_output_items = if !output_text.trim().is_empty() {
        let mut items = vec![json!({
            "type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": &output_text}]
        })];
        items.extend(output_items);
        items
    } else {
        output_items
    };

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
        output_items: final_output_items,
        usage,
        usage_hops,
        provider_key: provider_key.to_string(),
        model_profile: format!("{provider_key}/{model_id}"),
        model_id: model_id.to_string(),
        capabilities: ProviderCapabilities::chat_completions(),
    })
}

// ── Request Building ─────────────────────────────────────────────────────────

fn build_chat_payload(model_id: &str, req: &ResponseStreamRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let instructions = req.instructions_override.as_deref()
        .filter(|s| !s.trim().is_empty()).map(String::from);
    if let Some(ref instructions) = instructions {
        messages.push(json!({"role": "system", "content": instructions}));
    }
    messages.extend(input_items_to_messages(&req.input_items));

    let mut payload = json!({
        "model": model_id, "messages": messages, "stream": true,
        "stream_options": {"include_usage": true},
    });
    if let Some(ref effort) = req.reasoning_effort {
        let trimmed = effort.trim().to_lowercase();
        if !trimmed.is_empty() { payload["reasoning_effort"] = json!(trimmed); }
    }
    if let Some(ref tools) = req.tools { if !tools.is_empty() { payload["tools"] = Value::Array(tools.clone()); } }
    if let Some(ref tool_choice) = req.tool_choice { payload["tool_choice"] = tool_choice.clone(); }
    if let Some(ref text) = req.text {
        if let Some(format) = text.get("format") {
            match format.get("type").and_then(Value::as_str) {
                Some("json_object") => {
                    payload["response_format"] = json!({"type": "json_object"});
                }
                Some("json_schema") => {
                    let name = format.get("name")
                        .and_then(Value::as_str)
                        .unwrap_or("response");
                    let schema = format.get("schema")
                        .cloned()
                        .unwrap_or_else(|| json!({"type": "object"}));
                    let strict = format.get("strict")
                        .and_then(Value::as_bool)
                        .unwrap_or(true);
                    payload["response_format"] = json!({
                        "type": "json_schema",
                        "json_schema": {
                            "name": name,
                            "strict": strict,
                            "schema": schema,
                        }
                    });
                }
                _ => {}
            }
        }
    }
    // ── Sampling parameters ──
    if let Some(temperature) = req.temperature { payload["temperature"] = json!(temperature); }
    if let Some(top_p) = req.top_p { payload["top_p"] = json!(top_p); }
    if let Some(presence_penalty) = req.presence_penalty { payload["presence_penalty"] = json!(presence_penalty); }
    if let Some(frequency_penalty) = req.frequency_penalty { payload["frequency_penalty"] = json!(frequency_penalty); }
    if let Some(max_tokens) = req.max_tokens { payload["max_tokens"] = json!(max_tokens); }
    if let Some(max_completion_tokens) = req.max_completion_tokens { payload["max_completion_tokens"] = json!(max_completion_tokens); }
    if let Some(reasoning_budget) = req.reasoning_budget_tokens {
        // Chat Completions may not have a standard field for reasoning budget tokens.
        // Some providers (e.g. OpenAI o-series) accept it via reasoning_effort only.
        // We set it as a top-level field; gateways/vLLM may forward it.
        payload["reasoning_budget_tokens"] = json!(reasoning_budget);
    }
    payload
}

fn input_items_to_messages(items: &[Value]) -> Vec<Value> {
    let mut messages = Vec::new();
    for item in items {
        match item.get("type").and_then(Value::as_str).unwrap_or("") {
            "function_call" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let arguments = item.get("arguments")
                    .map(|a| if a.is_string() { a.as_str().unwrap_or("{}").to_string() } else { a.to_string() })
                    .unwrap_or_else(|| "{}".to_string());
                messages.push(json!({
                    "role": "assistant",
                    "tool_calls": [{"id": call_id, "type": "function", "function": {"name": name, "arguments": arguments}}]
                }));
            }
            "function_call_output" => {
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                    "content": item.get("output").and_then(Value::as_str).unwrap_or(""),
                }));
            }
            _ => {
                let role = match item.get("role").and_then(Value::as_str) {
                    Some("assistant") => "assistant",
                    Some("system") => "system",
                    Some("developer") => "developer",
                    _ => "user",
                };
                let content_value = item.get("content").cloned().unwrap_or(Value::Null);
                if content_value.is_null() { continue; }
                if let Some(arr) = content_value.as_array() {
                    let has_non_text = arr.iter().any(|part| {
                        !matches!(part.get("type").and_then(Value::as_str), Some("text") | None)
                    });
                    if has_non_text {
                        messages.push(json!({"role": role, "content": arr}));
                    } else {
                        let text = arr.iter()
                            .filter_map(|part| part.get("text").and_then(Value::as_str))
                            .collect::<Vec<_>>()
                            .join("");
                        if !text.trim().is_empty() {
                            messages.push(json!({"role": role, "content": text}));
                        }
                    }
                } else if let Some(text) = content_value.as_str() {
                    if !text.trim().is_empty() {
                        messages.push(json!({"role": role, "content": text}));
                    }
                }
            }
        }
    }
    messages
}

// ── SSE Event Processing ─────────────────────────────────────────────────────

#[allow(clippy::too_many_arguments)]
fn process_chat_event(
    event_block: &str,
    state: &mut ChatStreamState,
    response_id: &mut String,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    on_delta: &mut dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()>,
) -> AgentJaxResult<bool> {
    let data = event_block.lines()
        .filter(|line| line.starts_with("data:"))
        .map(|line| line[5..].trim_start())
        .collect::<Vec<_>>()
        .join("\n");

    if data.is_empty() || data == "[DONE]" { return Ok(data == "[DONE]"); }

    let value: Value = serde_json::from_str(&data)
        .map_err(|_| AgentJaxError::internal("Failed to parse Chat Completions SSE JSON"))?;
    if let Some(err) = value.get("error") {
        return Err(AgentJaxError::internal(format!("Chat Completions error: {err}")));
    }

    if let Some(id) = value.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) {
        *response_id = id.to_string();
    }
    if let Some(u) = parse_chat_usage(&value) {
        // Emit usage in real-time so the UI can show live token counts.
        on_delta(ProviderStreamEvent::UsageUpdated {
            response_id: response_id.clone(),
            usage: u.clone(),
            aggregate_usage: u.clone(),
        })?;
        *usage = Some(u);
    }

    if let Some(choices) = value.get("choices").and_then(Value::as_array) {
        for choice in choices {
            let delta = choice.get("delta").and_then(Value::as_object).cloned().unwrap_or_default();

            if let Some(content) = delta.get("reasoning_content").and_then(Value::as_str) {
                if !content.is_empty() {
                    if !state.reasoning_started {
                        state.reasoning_started = true;
                        on_delta(ProviderStreamEvent::ReasoningStarted)?;
                    }
                    on_delta(ProviderStreamEvent::ReasoningDelta { delta: content.to_string() })?;
                }
            }

            if let Some(content) = delta.get("content").and_then(Value::as_str) {
                if !content.is_empty() {
                    // If reasoning was streaming and now regular content begins,
                    // emit ReasoningCompleted before the first output text.
                    if state.reasoning_started {
                        state.reasoning_started = false;
                        on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None })?;
                    }
                    if !state.emitted_output_started {
                        state.emitted_output_started = true;
                        on_delta(ProviderStreamEvent::OutputTextStarted)?;
                    }
                    output_text.push_str(content);
                    on_delta(ProviderStreamEvent::OutputTextDelta { delta: content.to_string(), phase: None })?;
                }
            }

            if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                for tc in tool_calls {
                    let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                    let entry = state.tool_calls.entry(index).or_insert_with(|| ChatToolCallEntry {
                        item_id: format!("item_chat_{index}"),
                        call_id: tc.get("id").and_then(Value::as_str).unwrap_or("").to_string(),
                        name: String::new(),
                        arguments: String::new(),
                        started: false,
                        completed: false,
                    });
                    if let Some(id) = tc.get("id").and_then(Value::as_str).filter(|id| !id.is_empty()) { entry.call_id = id.to_string(); }
                    if let Some(name) = tc.pointer("/function/name").and_then(Value::as_str) { entry.name.push_str(name); }
                    if !entry.started && !entry.name.is_empty() {
                        entry.started = true;
                        on_delta(ProviderStreamEvent::ToolCallStarted {
                            item_id: entry.item_id.clone(), call_id: entry.call_id.clone(), name: entry.name.clone(), presentation: None,
                        })?;
                    }
                    if let Some(args) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                        entry.arguments.push_str(args);
                        on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                            item_id: entry.item_id.clone(), call_id: entry.call_id.clone(), delta: args.to_string(),
                        })?;
                    }
                }
            }

            if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                if finish_reason == "tool_calls" {
                    let entries: Vec<(String, String, String, String)> = state.tool_calls.values()
                        .filter(|e| !e.completed && !e.name.is_empty())
                        .map(|e| (e.item_id.clone(), e.call_id.clone(), e.name.clone(), e.arguments.clone()))
                        .collect();
                    for (item_id, call_id, name, arguments) in entries {
                        if let Some(entry) = state.tool_calls.values_mut().find(|e| e.call_id == call_id) { entry.completed = true; }
                        on_delta(ProviderStreamEvent::ToolCallCompleted {
                            item_id: item_id.clone(), call_id: call_id.clone(), name: name.clone(),
                            arguments: arguments.clone(), presentation: None,
                        })?;
                        output_items.push(json!({"type": "function_call", "id": item_id, "call_id": call_id, "name": name, "arguments": arguments}));
                    }
                }
                return Ok(matches!(finish_reason, "stop" | "length" | "content_filter"));
            }
        }
    }
    Ok(false)
}

fn parse_chat_usage(value: &Value) -> Option<ProviderUsage> {
    let usage: ProviderUsage = serde_json::from_value(value.get("usage")?.clone()).ok()?;
    (usage.prompt_tokens > 0 || usage.completion_tokens > 0 || usage.total_tokens > 0).then_some(usage)
}

struct ChatStreamState {
    emitted_output_started: bool,
    reasoning_started: bool,
    tool_calls: BTreeMap<usize, ChatToolCallEntry>,
}

struct ChatToolCallEntry {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    started: bool,
    completed: bool,
}

impl ChatStreamState {
    fn new() -> Self { Self { emitted_output_started: false, reasoning_started: false, tool_calls: BTreeMap::new() } }
}
