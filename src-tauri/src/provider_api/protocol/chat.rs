//! OpenAI Chat Completions API protocol implementation.

use crate::config::{AppConfig, ProviderConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::protocol::base_streaming;
use crate::provider_api::protocol::base_streaming::{
    finalize_response_id, run_sse_stream, setup_http_request, HasReasoningState, StreamStateMachine,
};
use crate::provider_api::types::*;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::watch;

/// Stream a response using the OpenAI Chat Completions API.
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
    let body = build_chat_payload(model_id, req);
    let response = setup_http_request(provider_key, provider_config, "/chat/completions", &body).await?;

    // ── Parse SSE stream via shared infrastructure ──
    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items: Vec<Value> = Vec::new();
    let mut usage: Option<ProviderUsage> = None;

    let mut state = ChatStreamState::new();
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

    let final_output_items = if !output_text.trim().is_empty() {
        let message_item = json!({
            "type": "message", "role": "assistant",
            "content": [{"type": "output_text", "text": &output_text}]
        });
        // Insert assistant message AFTER reasoning items but BEFORE
        // function_call items, preserving the logical order:
        //   reasoning → assistant text → function_calls
        // This matters for think models (DeepSeek R1, o-series) where
        // the model's chain-of-thought must precede output text, which
        // in turn precedes tool calls in the context stream.
        let split_idx = output_items
            .iter()
            .position(|item| {
                !matches!(
                    item.get("type").and_then(Value::as_str),
                    Some("reasoning")
                )
            })
            .unwrap_or(output_items.len());
        let mut items = Vec::with_capacity(output_items.len() + 1);
        items.extend_from_slice(&output_items[..split_idx]);
        items.push(message_item);
        items.extend_from_slice(&output_items[split_idx..]);
        items
    } else {
        output_items
    };

    let final_response_id = finalize_response_id(&response_id, provider_key);
    let usage_hops = base_streaming::build_usage_hops(&usage, &final_response_id);

    Ok(ResponseStreamResult {
        response_id: final_response_id,
        output_text,
        output_items: final_output_items,
        usage,
        usage_hops,
        provider_key: provider_key.to_string(),
        model_profile: format!("{provider_key}/{model_id}"),
        model_id: model_id.to_string(),
        capabilities: Default::default(),
    })
}

// ── Request Building ─────────────────────────────────────────────────────────

fn build_chat_payload(model_id: &str, req: &ResponseStreamRequest) -> Value {
    let mut messages: Vec<Value> = Vec::new();
    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|s| !s.trim().is_empty())
        .map(String::from);
    if let Some(ref instructions) = instructions {
        messages.push(json!({"role": "system", "content": instructions}));
    }
    messages.extend(input_items_to_messages(&req.input_items));

    let mut payload = json!({
        "model": model_id, "messages": messages, "stream": true,
        "stream_options": {"include_usage": true},
    });
    if let Some(ref config) = req.reasoning
        && config.enabled
            && let Some(ref effort) = config.effort {
                payload["reasoning_effort"] = json!(effort.as_str());
            }
    if let Some(ref tools) = req.tools
        && !tools.is_empty()
    {
        payload["tools"] = Value::Array(tools.clone());
    }
    if let Some(ref tool_choice) = req.tool_choice {
        payload["tool_choice"] = tool_choice.clone();
    }
    if let Some(ref text) = req.text
        && let Some(format) = text.get("format")
    {
        match format.get("type").and_then(Value::as_str) {
            Some("json_object") => {
                payload["response_format"] = json!({"type": "json_object"});
            }
            Some("json_schema") => {
                let name = format
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("response");
                let schema = format
                    .get("schema")
                    .cloned()
                    .unwrap_or_else(|| json!({"type": "object"}));
                let strict = format
                    .get("strict")
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
    // ── Sampling parameters ──
    if let Some(temperature) = req.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(top_p) = req.top_p {
        payload["top_p"] = json!(top_p);
    }
    if let Some(presence_penalty) = req.presence_penalty {
        payload["presence_penalty"] = json!(presence_penalty);
    }
    if let Some(frequency_penalty) = req.frequency_penalty {
        payload["frequency_penalty"] = json!(frequency_penalty);
    }
    if let Some(max_tokens) = req.max_tokens {
        payload["max_tokens"] = json!(max_tokens);
    }
    if let Some(max_completion_tokens) = req.max_completion_tokens {
        payload["max_completion_tokens"] = json!(max_completion_tokens);
    }
    if let Some(ref config) = req.reasoning
        && config.enabled
            && let Some(budget) = config.budget_tokens {
                payload["reasoning_budget_tokens"] = json!(budget);
            }

    // ── Extra body fields (provider-specific passthrough) ──
    for (key, value) in &req.extra_body {
        // Skip keys that are already set as standard parameters to avoid
        // overriding explicit fields with extra_body values.
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

/// Accumulates fields for a single Chat Completions assistant message.
///
/// Reasoning, function_call, and assistant-text items from the Responses-API
/// item stream all map to `role: "assistant"` in Chat Completions. Consecutive
/// assistant-typed items must be merged into ONE message — the Chat Completions
/// API rejects consecutive assistant messages, and DeepSeek's thinking mode
/// requires `reasoning_content` to appear on the same message as `content`
/// and/or `tool_calls`.
struct AssistantBuilder {
    reasoning_content: Option<String>,
    content: Option<String>,
    tool_calls: Vec<Value>,
}

impl AssistantBuilder {
    fn new() -> Self {
        Self {
            reasoning_content: None,
            content: None,
            tool_calls: Vec::new(),
        }
    }

    fn is_empty(&self) -> bool {
        self.reasoning_content.is_none() && self.content.is_none() && self.tool_calls.is_empty()
    }

    fn into_message(self) -> Value {
        let mut msg = serde_json::Map::new();
        msg.insert("role".to_string(), json!("assistant"));

        let content = self.content.unwrap_or_default();
        msg.insert("content".to_string(), json!(content));

        if let Some(rc) = self.reasoning_content {
            msg.insert("reasoning_content".to_string(), json!(rc));
        }
        if !self.tool_calls.is_empty() {
            msg.insert("tool_calls".to_string(), Value::Array(self.tool_calls));
        }

        Value::Object(msg)
    }
}

/// Extract a flat text string from a content value that may be a JSON array of
/// `{"type":"text"/"input_text"/"output_text","text":"..."}` parts, or a plain
/// string. Returns `None` when the content carries non-text parts (images,
/// files, etc.) — those are handled separately by the caller.
fn extract_flat_text(content_value: &Value) -> Option<String> {
    if let Some(arr) = content_value.as_array() {
        let has_non_text = arr.iter().any(|part| {
            let ptype = part.get("type").and_then(Value::as_str);
            !matches!(ptype, Some("text") | Some("input_text") | Some("output_text") | None)
        });
        if has_non_text {
            return None;
        }
        let text = arr
            .iter()
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        if text.trim().is_empty() {
            return None;
        }
        Some(text)
    } else if let Some(text) = content_value.as_str() {
        if text.trim().is_empty() {
            return None;
        }
        Some(text.to_string())
    } else {
        None
    }
}

/// Normalize a content array in-place: rewrites `input_text` / `output_text`
/// part types to `text` so Chat Completions providers understand them.
fn normalize_content_parts(arr: &[Value]) -> Vec<Value> {
    arr.iter()
        .map(|part| {
            let mut p = part.clone();
            match p.get("type").and_then(Value::as_str) {
                Some("input_text") | Some("output_text") => {
                    p["type"] = json!("text");
                }
                _ => {}
            }
            p
        })
        .collect()
}

fn input_items_to_messages(items: &[Value]) -> Vec<Value> {
    let mut messages: Vec<Value> = Vec::new();
    // Consecutive assistant-typed items (reasoning, function_call,
    // assistant-text) are merged into a single assistant message so the
    // Chat Completions API never sees two assistant messages in a row.
    let mut pending: Option<AssistantBuilder> = None;

    /// Finalize and push the pending assistant message (if any).
    macro_rules! flush_pending {
        () => {
            if let Some(p) = pending.take() {
                if !p.is_empty() {
                    messages.push(p.into_message());
                }
            }
        };
    }

    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "function_call" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                let arguments = item
                    .get("arguments")
                    .map(|a| {
                        if a.is_string() {
                            a.as_str().unwrap_or("{}").to_string()
                        } else {
                            a.to_string()
                        }
                    })
                    .unwrap_or_else(|| "{}".to_string());
                let p = pending.get_or_insert_with(AssistantBuilder::new);
                p.tool_calls.push(json!({
                    "id": call_id,
                    "type": "function",
                    "function": {"name": name, "arguments": arguments}
                }));
            }
            "function_call_output" => {
                // Tool outputs always end the assistant block — flush any
                // pending assistant before pushing the tool-role message.
                flush_pending!();
                messages.push(json!({
                    "role": "tool",
                    "tool_call_id": item.get("call_id").and_then(Value::as_str).unwrap_or(""),
                    "content": item.get("output").and_then(Value::as_str).unwrap_or(""),
                }));
            }
            "reasoning" => {
                // Reasoning / thinking content from CoT models (DeepSeek R1,
                // OpenAI o-series). Must be merged into the SAME assistant
                // message as any following `content` or `tool_calls` — the
                // Chat Completions API rejects consecutive assistant messages
                // and DeepSeek requires `reasoning_content` on the message
                // that carries the tool calls / output text.
                //
                // When multiple reasoning blocks appear (accumulated from
                // multiple hops), append rather than overwrite so no
                // intermediate thinking content is lost.
                let text = item.get("text").and_then(Value::as_str).unwrap_or("");
                if !text.trim().is_empty() {
                    let p = pending.get_or_insert_with(AssistantBuilder::new);
                    match &mut p.reasoning_content {
                        Some(existing) => {
                            existing.push('\n');
                            existing.push_str(text);
                        }
                        None => {
                            p.reasoning_content = Some(text.to_string());
                        }
                    }
                }
            }
            _ => {
                // Role-based items (user, system, assistant).
                let role = match item.get("role").and_then(Value::as_str) {
                    Some("assistant") => "assistant",
                    Some("system") | Some("developer") => {
                        // 'developer' was an OpenAI Responses API-specific
                        // distinction that has been removed — kept as a
                        // fallback for any persisted data that may still
                        // use it. Both map to Chat Completions 'system'.
                        "system"
                    }
                    _ => "user",
                };

                let content_value = item.get("content").cloned().unwrap_or(Value::Null);
                if content_value.is_null() {
                    continue;
                }

                if role == "assistant" {
                    // Assistant-text items merge with any pending assistant
                    // (e.g. a `reasoning` item that arrived just before).
                    let p = pending.get_or_insert_with(AssistantBuilder::new);
                    if let Some(text) = extract_flat_text(&content_value) {
                        p.content = Some(text);
                    } else if let Some(arr) = content_value.as_array() {
                        // Non-text content (images, files) — rare for
                        // assistant history. Push standalone.
                        flush_pending!();
                        messages.push(json!({
                            "role": "assistant",
                            "content": normalize_content_parts(arr),
                        }));
                    }
                } else {
                    // Non-assistant role (user / system): finalize any
                    // pending assistant before pushing.
                    flush_pending!();
                    if let Some(text) = extract_flat_text(&content_value) {
                        messages.push(json!({"role": role, "content": text}));
                    } else if let Some(arr) = content_value.as_array() {
                        messages.push(json!({
                            "role": role,
                            "content": normalize_content_parts(arr),
                        }));
                    }
                }
            }
        }
    }

    // Finalize any remaining pending assistant at end of items.
    flush_pending!();

    messages
}

// ── SSE Event Processing ─────────────────────────────────────────────────────

fn parse_chat_usage(value: &Value) -> Option<ProviderUsage> {
    let usage: ProviderUsage = serde_json::from_value(value.get("usage")?.clone()).ok()?;
    base_streaming::has_nonzero_usage(&usage).then_some(usage)
}

struct ChatStreamState {
    emitted_output_started: bool,
    reasoning_started: bool,
    reasoning_buffer: String,
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
    fn new() -> Self {
        Self {
            emitted_output_started: false,
            reasoning_started: false,
            reasoning_buffer: String::new(),
            tool_calls: BTreeMap::new(),
        }
    }
}

impl HasReasoningState for ChatStreamState {
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

impl StreamStateMachine for ChatStreamState {
    fn process_event(
        &mut self,
        event_block: &str,
        response_id: &mut String,
        output_text: &mut String,
        output_items: &mut Vec<Value>,
        usage: &mut Option<ProviderUsage>,
        on_delta: &mut dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()>,
    ) -> AgentJaxResult<bool> {
        let data = event_block
            .lines()
            .filter(|line| line.starts_with("data:"))
            .map(|line| line[5..].trim_start())
            .collect::<Vec<_>>()
            .join("\n");

        if data.is_empty() || data == "[DONE]" {
            return Ok(data == "[DONE]");
        }

        let value: Value = serde_json::from_str(&data)
            .map_err(|_| AgentJaxError::internal("Failed to parse Chat Completions SSE JSON"))?;
        if let Some(err) = value.get("error") {
            return Err(AgentJaxError::internal(format!(
                "Chat Completions error: {err}"
            )));
        }

        if let Some(id) = value
            .get("id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
        {
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
                let delta = choice
                    .get("delta")
                    .and_then(Value::as_object)
                    .cloned()
                    .unwrap_or_default();

                if let Some(content) = delta.get("reasoning_content").and_then(Value::as_str)
                    && !content.is_empty()
                {
                    if !self.reasoning_started {
                        self.reasoning_started = true;
                        on_delta(ProviderStreamEvent::ReasoningStarted)?;
                    }
                    on_delta(ProviderStreamEvent::ReasoningDelta {
                        delta: content.to_string(),
                    })?;
                    self.reasoning_buffer.push_str(content);
                }

                if let Some(content) = delta.get("content").and_then(Value::as_str)
                    && !content.is_empty()
                {
                    // If reasoning was streaming and now regular content begins,
                    // emit ReasoningCompleted before the first output text.
                    if self.reasoning_started {
                        self.reasoning_started = false;
                        on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None })?;
                        // Flush accumulated reasoning into output_items.
                        if !self.reasoning_buffer.is_empty() {
                            output_items.push(json!({
                                "type": "reasoning",
                                "text": self.reasoning_buffer.clone(),
                            }));
                            self.reasoning_buffer.clear();
                        }
                    }
                    if !self.emitted_output_started {
                        self.emitted_output_started = true;
                        on_delta(ProviderStreamEvent::OutputTextStarted)?;
                    }
                    output_text.push_str(content);
                    on_delta(ProviderStreamEvent::OutputTextDelta {
                        delta: content.to_string(),
                        phase: None,
                    })?;
                }

                if let Some(tool_calls) = delta.get("tool_calls").and_then(Value::as_array) {
                    for tc in tool_calls {
                        let index = tc.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                        let entry =
                            self
                                .tool_calls
                                .entry(index)
                                .or_insert_with(|| ChatToolCallEntry {
                                    item_id: format!("item_chat_{index}"),
                                    call_id: tc
                                        .get("id")
                                        .and_then(Value::as_str)
                                        .unwrap_or("")
                                        .to_string(),
                                    name: String::new(),
                                    arguments: String::new(),
                                    started: false,
                                    completed: false,
                                });
                        if let Some(id) = tc
                            .get("id")
                            .and_then(Value::as_str)
                            .filter(|id| !id.is_empty())
                        {
                            entry.call_id = id.to_string();
                        }
                        if let Some(name) = tc.pointer("/function/name").and_then(Value::as_str) {
                            entry.name.push_str(name);
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
                        if let Some(args) = tc.pointer("/function/arguments").and_then(Value::as_str) {
                            entry.arguments.push_str(args);
                            on_delta(ProviderStreamEvent::ToolCallArgumentsDelta {
                                item_id: entry.item_id.clone(),
                                call_id: entry.call_id.clone(),
                                delta: args.to_string(),
                            })?;
                        }
                    }
                }

                if let Some(finish_reason) = choice.get("finish_reason").and_then(Value::as_str) {
                    // Flush any accumulated reasoning before processing tool_calls / stop.
                    if self.reasoning_started && !self.reasoning_buffer.is_empty() {
                        self.reasoning_started = false;
                        on_delta(ProviderStreamEvent::ReasoningCompleted { total_tokens: None })?;
                        output_items.push(json!({
                            "type": "reasoning",
                            "text": self.reasoning_buffer.clone(),
                        }));
                        self.reasoning_buffer.clear();
                    }
                    if finish_reason == "tool_calls" {
                        let entries: Vec<(String, String, String, String)> = self
                            .tool_calls
                            .values()
                            .filter(|e| !e.completed && !e.name.is_empty())
                            .map(|e| {
                                (
                                    e.item_id.clone(),
                                    e.call_id.clone(),
                                    e.name.clone(),
                                    e.arguments.clone(),
                                )
                            })
                            .collect();
                        for (item_id, call_id, name, arguments) in entries {
                            if let Some(entry) =
                                self.tool_calls.values_mut().find(|e| e.call_id == call_id)
                            {
                                entry.completed = true;
                            }
                            on_delta(ProviderStreamEvent::ToolCallCompleted {
                                item_id: item_id.clone(),
                                call_id: call_id.clone(),
                                name: name.clone(),
                                arguments: arguments.clone(),
                                presentation: None,
                            })?;
                            output_items.push(json!({"type": "function_call", "id": item_id, "call_id": call_id, "name": name, "arguments": arguments}));
                        }
                    }
                    // Do NOT return true here to terminate the stream.
                    // When stream_options: {include_usage: true} is set, the API
                    // sends a separate usage chunk AFTER the finish_reason chunk
                    // but BEFORE the [DONE] marker. Terminating on finish_reason
                    // would cause the usage data to be lost.
                    // The stream is instead terminated by [DONE] or stream end.
                }
            }
        }
        Ok(false)
    }
}
