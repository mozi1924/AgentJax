//! OpenAI Responses API protocol implementation.

use crate::config::{AppConfig, ProviderConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::protocol::base_streaming;
use crate::provider_api::protocol::base_streaming::{
    finalize_response_id, run_sse_stream, setup_http_request, StreamStateMachine,
};
use crate::provider_api::protocol::base_streaming::HasReasoningState;
use crate::provider_api::types::*;
use serde_json::{Value, json};
use tokio::sync::watch;

/// Stream a response using the OpenAI Responses API.
pub async fn stream_response<F>(
    _config: &AppConfig,
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
    let body = build_response_payload(model_id, req);
    let response = setup_http_request(provider_key, provider_config, "/responses", &body).await?;

    // ── Parse SSE stream via shared infrastructure ──
    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items: Vec<Value> = Vec::new();
    let mut usage: Option<ProviderUsage> = None;

    let mut state = ResponsesStreamState::new();
    state = run_sse_stream(
        response,
        state,
        cancel_rx,
        &mut response_id,
        &mut output_text,
        &mut output_items,
        &mut usage,
        &mut on_delta,
    )
    .await?;

    // Flush any remaining reasoning (e.g. stream ended mid-reasoning).
    state.flush_remaining_reasoning(&mut output_items, &mut on_delta)?;

    let final_response_id = finalize_response_id(&response_id, provider_key);
    let usage_hops = base_streaming::build_usage_hops(&usage, &final_response_id);

    Ok(ResponseStreamResult {
        response_id: final_response_id,
        output_text,
        output_items,
        usage,
        usage_hops,
        provider_key: provider_key.to_string(),
        model_profile: format!("{provider_key}/{model_id}"),
        model_id: model_id.to_string(),
        capabilities: Default::default(),
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

    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(String::from);
    if let Some(ref instructions) = instructions {
        payload["instructions"] = json!(instructions);
    }
    if let Some(ref config) = req.reasoning {
        if config.enabled {
            if let Some(ref effort) = config.effort {
                payload["reasoning"] = json!({ "effort": effort.as_str() });
            }
        }
    }
    if let Some(ref tools) = req.tools
        && !tools.is_empty()
    {
        payload["tools"] = Value::Array(tools.clone());
    }
    if let Some(ref tool_choice) = req.tool_choice {
        payload["tool_choice"] = tool_choice.clone();
    }
    if let Some(ref text) = req.text {
        payload["text"] = text.clone();
    }
    if let Some(ref include) = req.include
        && !include.is_empty()
    {
        payload["include"] = Value::Array(include.iter().map(|s| json!(s)).collect());
    }

    // ── Extra body fields (provider-specific passthrough) ──
    for (key, value) in &req.extra_body {
        if !payload
            .as_object()
            .map(|o| o.contains_key(key))
            .unwrap_or(false)
        {
            payload[key] = value.clone();
        }
    }
    payload
}

fn normalize_input_items(items: &[Value]) -> Value {
    let normalized: Vec<Value> = items
        .iter()
        .filter(|item| {
            // Filter out reasoning items — the OpenAI Responses API does not
            // accept them as input. Reasoning context from previous hops is
            // implicitly available through the API's internal continuation
            // mechanism; sending them explicitly would cause an error.
            item.get("type").and_then(Value::as_str) != Some("reasoning")
        })
        .map(|item| {
            let mut cloned = item.clone();
            if let Some(obj) = cloned.as_object_mut() {
                let item_type = obj.get("type").and_then(Value::as_str).unwrap_or("");
                if item_type != "function_call" && item_type != "function_call_output" {
                    obj.remove("id");
                }
                let role = obj
                    .get("role")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if let Some(content) = obj.get_mut("content").and_then(|v| v.as_array_mut()) {
                    for part in content.iter_mut() {
                        let part_type = part
                            .get("type")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        if matches!(part_type.as_str(), "text" | "input_text" | "output_text") {
                            part["type"] = json!(if role == "assistant" {
                                "output_text"
                            } else {
                                "input_text"
                            });
                        }
                    }
                }
            }
            cloned
        })
        .collect();
    Value::Array(normalized)
}

// ── SSE Event Processing ─────────────────────────────────────────────────────

fn extract_sse_data(event_block: &str) -> String {
    let trimmed = event_block.trim();
    if trimmed.starts_with('{') || trimmed.starts_with('[') {
        return trimmed.to_string();
    }
    trimmed
        .lines()
        .filter(|line| line.starts_with("data:"))
        .map(|line| line[5..].trim_start())
        .collect::<Vec<_>>()
        .join("\n")
}

fn parse_responses_usage(value: &Value) -> Option<ProviderUsage> {
    let usage_value = value
        .pointer("/response/usage")
        .or_else(|| value.get("usage"))
        .unwrap_or(value);
    let usage: ProviderUsage = serde_json::from_value(usage_value.clone()).ok()?;
    base_streaming::has_nonzero_usage(&usage).then_some(usage)
}

struct ResponsesStreamState {
    emitted_output_started: bool,
    reasoning_started: bool,
    reasoning_buffer: String,
    completed_tool_calls: Vec<String>,
}

impl ResponsesStreamState {
    fn new() -> Self {
        Self {
            emitted_output_started: false,
            reasoning_started: false,
            reasoning_buffer: String::new(),
            completed_tool_calls: Vec::new(),
        }
    }
}

impl HasReasoningState for ResponsesStreamState {
    fn reasoning_started(&self) -> bool {
        self.reasoning_started
    }
    fn reasoning_buffer(&self) -> &str {
        &self.reasoning_buffer
    }
    fn set_reasoning_started(&mut self, val: bool) {
        self.reasoning_started = val;
    }
    fn take_reasoning_buffer(&mut self) -> String {
        std::mem::take(&mut self.reasoning_buffer)
    }
}

impl StreamStateMachine for ResponsesStreamState {
    fn process_event(
        &mut self,
        event_block: &str,
        response_id: &mut String,
        output_text: &mut String,
        output_items: &mut Vec<Value>,
        usage: &mut Option<ProviderUsage>,
        on_delta: &mut dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()>,
    ) -> AgentJaxResult<bool> {
        let data = extract_sse_data(event_block);
        if data.is_empty() || data == "[DONE]" {
            return Ok(data == "[DONE]");
        }

        let value: Value = serde_json::from_str(&data)
            .map_err(|_| AgentJaxError::internal("Failed to parse SSE JSON"))?;
        if let Some(err) = value.get("error") {
            return Err(AgentJaxError::internal(format!(
                "Responses API error: {err}"
            )));
        }

        if let Some(id) = value
            .pointer("/response/id")
            .or_else(|| value.get("response_id"))
            .or_else(|| value.get("id"))
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
            *response_id = id.to_string();
        }

        if let Some(u) = parse_responses_usage(&value) {
            *usage = Some(u);
        }

        let type_str = value.get("type").and_then(Value::as_str).unwrap_or("");
        let done = type_str == "response.completed" || type_str == "response.done";

        match type_str {
            "response.output_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str)
                    && !delta.is_empty()
                {
                    if !self.emitted_output_started {
                        self.emitted_output_started = true;
                        on_delta(ProviderStreamEvent::OutputTextStarted)?;
                    }
                    output_text.push_str(delta);
                    on_delta(ProviderStreamEvent::OutputTextDelta {
                        delta: delta.to_string(),
                        phase: None,
                    })?;
                }
            }
            "response.output_item.added" => {
                if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("function_call")
                {
                    on_delta(ProviderStreamEvent::ToolCallStarted {
                        item_id: item
                            .get("id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        call_id: item
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        name: item
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        presentation: None,
                    })?;
                }
            }
            "response.function_call_arguments.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str) {
                    on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                        item_id: value
                            .get("item_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        call_id: value
                            .get("call_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        delta: delta.to_string(),
                    })?;
                }
            }
            "response.function_call_arguments.done" => {
                if let Some(call_id) = value.get("call_id").and_then(Value::as_str) {
                    self.completed_tool_calls.push(call_id.to_string());
                    on_delta(ProviderStreamEvent::ToolCallCompleted {
                        item_id: value
                            .get("item_id")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        call_id: call_id.to_string(),
                        name: value
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                        arguments: value
                            .get("arguments")
                            .and_then(Value::as_str)
                            .unwrap_or("{}")
                            .to_string(),
                        presentation: None,
                    })?;
                }
            }
            "response.output_item.done" => {
                if let Some(item) = value.get("item")
                    && item.get("type").and_then(Value::as_str) == Some("message")
                {
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
                            phase: None,
                            response_id: response_id.clone(),
                        })?;
                    }
                }
            }
            // ── Reasoning / thinking events ──────────────────────────
            "response.reasoning.summary_part.added" if !self.reasoning_started => {
                self.reasoning_started = true;
                on_delta(ProviderStreamEvent::ReasoningStarted)?;
            }
            "response.reasoning.summary_text.delta" => {
                if let Some(delta) = value.get("delta").and_then(Value::as_str)
                    && !delta.is_empty()
                {
                    if !self.reasoning_started {
                        self.reasoning_started = true;
                        on_delta(ProviderStreamEvent::ReasoningStarted)?;
                    }
                    self.reasoning_buffer.push_str(delta);
                    on_delta(ProviderStreamEvent::ReasoningDelta {
                        delta: delta.to_string(),
                    })?;
                }
            }
            "response.reasoning.summary_part.done"
                if self.reasoning_started && !self.reasoning_buffer.is_empty() =>
            {
                self.reasoning_started = false;
                let total_tokens = value
                    .get("total_tokens")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens })?;
                output_items.push(json!({
                    "type": "reasoning",
                    "text": self.reasoning_buffer.clone(),
                }));
                self.reasoning_buffer.clear();
            }
            "response.completed" | "response.done"
                if self.reasoning_started && !self.reasoning_buffer.is_empty() =>
            {
                self.reasoning_started = false;
                on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None })?;
                output_items.push(json!({
                    "type": "reasoning",
                    "text": self.reasoning_buffer.clone(),
                }));
                self.reasoning_buffer.clear();
            }
            _ => {}
        }
        Ok(done)
    }
}
