use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::watch;

use super::core::ProviderIdFactory;
use super::registry;
use super::responses;
use super::sse::{split_sse_event_block, sse_data_payload};
use super::types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ProviderStreamEvent,
    ProviderUsage, ProviderUsageRecord, ResponseStreamRequest, ResponseStreamResult,
};
use crate::config::{ModelRequestConfig, ResolvedModelConfig};

#[derive(Debug, Clone, Default)]
struct AnthropicToolBlock {
    item_id: String,
    call_id: String,
    name: String,
    arguments_json: String,
    completed: bool,
}

#[derive(Debug, Clone, Default)]
struct AnthropicPayloadMessages {
    system_sections: Vec<String>,
    messages: Vec<Value>,
}

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let endpoint = format!(
        "{}/models",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );
    let endpoint =
        responses::http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params)
            .map_err(|e| format!("Failed to build Anthropic models endpoint URL: {e}"))?;
    let headers =
        build_request_headers(resolved, resolved.provider.resolved_credential().as_deref());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request = responses::http::apply_headers_to_reqwest(client.get(endpoint), &headers)
        .map_err(|e| format!("Failed to prepare Anthropic models request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Anthropic models API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Anthropic models API error ({status}): {text}"));
    }

    let root: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Anthropic model list JSON: {e}"))?;
    let models = parse_model_descriptors(&root);
    if models.is_empty() {
        return Err("Anthropic model list did not contain recognized model ids".to_string());
    }
    Ok(models)
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential();
    let headers = build_request_headers(resolved, credential.as_deref());
    let endpoint = format!(
        "{}/messages",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );
    let endpoint =
        responses::http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params)
            .map_err(|e| format!("Failed to build Anthropic messages endpoint URL: {e}"))?;
    let body = build_anthropic_payload(resolved, req)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request =
        responses::http::apply_headers_to_reqwest(client.post(endpoint).json(&body), &headers)
            .map_err(|e| format!("Failed to prepare Anthropic request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Anthropic API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Anthropic API error ({status}): {text}"));
    }

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items = Vec::new();
    let mut usage: Option<ProviderUsage> = None;
    let mut emitted_output_started = false;
    let mut id_factory = ProviderIdFactory::new("anthropic");
    let mut tool_blocks_by_index: BTreeMap<usize, AnthropicToolBlock> = BTreeMap::new();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    loop {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    break;
                }
            }
            next_chunk = stream.next() => {
                let Some(next_chunk) = next_chunk else {
                    break;
                };
                let bytes = next_chunk.map_err(|e| format!("Failed to read Anthropic stream: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    process_anthropic_event(
                        &event_block,
                        &mut response_id,
                        &mut output_text,
                        &mut output_items,
                        &mut usage,
                        &mut emitted_output_started,
                        &mut id_factory,
                        &mut tool_blocks_by_index,
                        on_delta,
                    )?;
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        process_anthropic_event(
            &buffer,
            &mut response_id,
            &mut output_text,
            &mut output_items,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_blocks_by_index,
            on_delta,
        )?;
    }
    finalize_all_tool_blocks(&mut tool_blocks_by_index, &mut output_items, on_delta)?;

    if !output_text.trim().is_empty() {
        output_items.insert(
            0,
            json!({
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": output_text
                }]
            }),
        );
    }
    if response_id.is_empty() {
        response_id = id_factory.response_id().to_string();
    }

    let usage_hops: Vec<ProviderUsageRecord> = usage
        .clone()
        .map(|usage| ProviderUsageRecord {
            response_id: response_id.clone(),
            usage,
        })
        .into_iter()
        .collect();

    Ok(ResponseStreamResult {
        response_id,
        output_text,
        output_items,
        usage,
        usage_hops,
        provider_key: resolved.provider_key.clone(),
        model_profile: resolved.profile_key.clone(),
        model_id: resolved.model_id.clone(),
        capabilities: registry::provider_capabilities("anthropic")
            .unwrap_or_else(|| registry::default_provider_definition().capabilities),
    })
}

pub fn get_reasoning_capability(
    _model_id: &str,
    cached_levels: Option<&[String]>,
) -> ModelReasoningCapability {
    let supported_reasoning_levels = cached_levels
        .map(responses::normalize_reasoning_levels)
        .filter(|levels| !levels.is_empty())
        .unwrap_or_default();

    ModelReasoningCapability {
        supports_reasoning: !supported_reasoning_levels.is_empty(),
        supported_reasoning_levels,
    }
}

fn build_request_headers(
    resolved: &ResolvedModelConfig,
    credential: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = responses::http::merge_request_headers(
        &[
            ("Content-Type", "application/json"),
            ("anthropic-version", "2023-06-01"),
        ],
        &resolved.provider,
        None,
        None,
    );

    if !has_header_case_insensitive(&headers, "x-api-key")
        && !has_header_case_insensitive(&headers, "Authorization")
    {
        if let Some(credential) = credential.map(str::trim).filter(|value| !value.is_empty()) {
            headers.insert("x-api-key".to_string(), credential.to_string());
        }
    }

    headers
}

fn has_header_case_insensitive(headers: &BTreeMap<String, String>, name: &str) -> bool {
    headers.keys().any(|key| key.eq_ignore_ascii_case(name))
}

fn build_anthropic_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Value, String> {
    let AnthropicPayloadMessages {
        mut system_sections,
        messages,
    } = build_anthropic_messages(req)?;
    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt);
    if !instructions.trim().is_empty() {
        system_sections.insert(0, instructions.trim().to_string());
    }

    let mut payload = json!({
        "model": resolved.model_id,
        "max_tokens": resolved.request.max_output_tokens.unwrap_or(4096),
        "messages": messages,
        "stream": true
    });
    if !system_sections.is_empty() {
        payload["system"] = Value::String(system_sections.join("\n\n"));
    }
    apply_generation_config(&mut payload, &resolved.request);
    if let Some(tools) = req.tools.as_ref().filter(|tools| !tools.is_empty()) {
        payload["tools"] = json!(tools);
        if let Some(tool_choice) = normalize_tool_choice(req.tool_choice.as_ref()) {
            payload["tool_choice"] = tool_choice;
        }
    }
    apply_extra_body(&mut payload, &resolved.request);
    Ok(payload)
}

fn apply_generation_config(payload: &mut Value, request: &ModelRequestConfig) {
    if let Some(temperature) = request.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        payload["top_p"] = json!(top_p);
    }
    if let Some(top_k) = request.top_k {
        payload["top_k"] = json!(top_k);
    }
}

fn normalize_tool_choice(tool_choice: Option<&Value>) -> Option<Value> {
    match tool_choice? {
        // Runtime requests use the OpenAI-compatible string form. Anthropic
        // expects a typed object, so normalize common choices at the adapter
        // boundary before the payload leaves AgentJax.
        Value::String(choice) => {
            let choice = choice.trim().to_ascii_lowercase();
            if choice.is_empty() {
                None
            } else if choice == "required" {
                Some(json!({ "type": "any" }))
            } else {
                Some(json!({ "type": choice }))
            }
        }
        Value::Object(obj) if !obj.is_empty() => Some(Value::Object(obj.clone())),
        _ => None,
    }
}

fn apply_extra_body(payload: &mut Value, request: &ModelRequestConfig) {
    for (key, value) in &request.extra_body {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if matches!(
            normalized.as_str(),
            "model" | "messages" | "system" | "stream" | "tools"
        ) {
            continue;
        }
        payload[key] = value.clone();
    }
}

fn build_anthropic_messages(
    req: &ResponseStreamRequest,
) -> Result<AnthropicPayloadMessages, String> {
    let mut out = AnthropicPayloadMessages::default();

    for item in &req.input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "function_call" | "custom_tool_call" => {
                if let Some(message) = anthropic_tool_use_message(item)? {
                    out.messages.push(message);
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                if let Some(message) = anthropic_tool_result_message(item) {
                    out.messages.push(message);
                }
            }
            "reasoning" => {
                if let Some(text) = extract_reasoning_summary(item) {
                    out.messages.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "text", "text": text }]
                    }));
                }
            }
            _ => {
                if let Some((system_text, message)) = anthropic_message_from_item(item)? {
                    if !system_text.trim().is_empty() {
                        out.system_sections.push(system_text);
                    }
                    if let Some(message) = message {
                        out.messages.push(message);
                    }
                }
            }
        }
    }

    Ok(out)
}

fn anthropic_message_from_item(item: &Value) -> Result<Option<(String, Option<Value>)>, String> {
    let raw_role = item
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw_role.is_empty() {
        return Ok(None);
    }
    let content = anthropic_content_parts(item)?;
    if content.is_empty() {
        return Ok(None);
    }
    if matches!(raw_role, "system" | "developer") {
        return Ok(Some((text_from_anthropic_parts(&content), None)));
    }
    let role = if raw_role == "assistant" {
        "assistant"
    } else {
        "user"
    };
    Ok(Some((
        String::new(),
        Some(json!({
            "role": role,
            "content": content
        })),
    )))
}

fn anthropic_tool_use_message(item: &Value) -> Result<Option<Value>, String> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    if call_id.trim().is_empty() || name.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({
        "role": "assistant",
        "content": [{
            "type": "tool_use",
            "id": call_id,
            "name": name,
            "input": parse_arguments_object(item.get("arguments"))
        }]
    })))
}

fn anthropic_tool_result_message(item: &Value) -> Option<Value> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    if call_id.trim().is_empty() {
        return None;
    }
    Some(json!({
        "role": "user",
        "content": [{
            "type": "tool_result",
            "tool_use_id": call_id,
            "content": item.get("output").and_then(Value::as_str).unwrap_or("")
        }]
    }))
}

fn anthropic_content_parts(item: &Value) -> Result<Vec<Value>, String> {
    let Some(content) = item.get("content") else {
        return Ok(Vec::new());
    };
    if let Some(text) = content.as_str() {
        return Ok((!text.trim().is_empty())
            .then(|| vec![json!({ "type": "text", "text": text })])
            .unwrap_or_default());
    }

    let Some(parts) = content.as_array() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for part in parts {
        if let Some(text) = part
            .get("text")
            .or_else(|| part.get("input_text"))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                out.push(json!({ "type": "text", "text": text }));
            }
            continue;
        }
        if let Some(image) = anthropic_image_part(part) {
            out.push(image);
        }
    }
    Ok(out)
}

fn anthropic_image_part(part: &Value) -> Option<Value> {
    let url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })
        .or_else(|| part.get("image").and_then(Value::as_str))?;

    if let Some(data) = url.strip_prefix("data:") {
        let (media_type, encoded) = data.split_once(";base64,")?;
        return Some(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": encoded
            }
        }));
    }

    Some(json!({
        "type": "image",
        "source": {
            "type": "url",
            "url": url
        }
    }))
}

fn text_from_anthropic_parts(parts: &[Value]) -> String {
    parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn parse_arguments_object(arguments: Option<&Value>) -> Value {
    let Some(arguments) = arguments else {
        return json!({});
    };
    if arguments.is_object() {
        return arguments.clone();
    }
    if let Some(arguments) = arguments.as_str() {
        return serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    }
    json!({})
}

fn extract_reasoning_summary(item: &Value) -> Option<String> {
    let summary = item.get("summary")?;
    if let Some(text) = summary.as_str() {
        return (!text.trim().is_empty()).then(|| text.trim().to_string());
    }
    let parts = summary.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}

#[allow(clippy::too_many_arguments)]
fn process_anthropic_event(
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

fn finalize_all_tool_blocks(
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

fn parse_model_descriptors(root: &Value) -> Vec<ProviderModelDescriptor> {
    let mut out = Vec::new();
    if let Some(items) = root.get("data").and_then(Value::as_array) {
        for item in items {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("model").and_then(Value::as_str))
                .unwrap_or("")
                .trim();
            if !id.is_empty() {
                out.push(ProviderModelDescriptor {
                    id: id.to_string(),
                    supported_reasoning_levels: Vec::new(),
                });
            }
        }
    }
    out
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
    use super::{build_anthropic_payload, process_anthropic_event};
    use crate::config::{
        ModelRequestConfig, PromptComposerConfig, ProviderConfig, ResolvedModelConfig,
        compile_prompt_composer,
    };
    use crate::providers::core::ProviderIdFactory;
    use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    fn test_resolved() -> ResolvedModelConfig {
        let prompt_assembly = compile_prompt_composer(&PromptComposerConfig::default());
        ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "anthropic".to_string(),
            provider: ProviderConfig::default(),
            model_id: "claude-sonnet-4-5".to_string(),
            model_ref: "anthropic/claude-sonnet-4-5".to_string(),
            system_prompt: "system prompt".to_string(),
            prompt_assembly,
            request: ModelRequestConfig {
                max_output_tokens: Some(1024),
                ..Default::default()
            },
            timeout_seconds: 60,
        }
    }

    #[test]
    fn payload_converts_timeline_to_anthropic_messages() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![
                json!({
                    "role": "developer",
                    "content": [{"type":"input_text","text":"dev note"}]
                }),
                json!({
                    "role": "user",
                    "content": [{"type":"input_text","text":"hello"}]
                }),
                json!({
                    "type":"function_call",
                    "call_id":"toolu_1",
                    "name":"lookup",
                    "arguments":{"q":"agent"}
                }),
                json!({
                    "type":"function_call_output",
                    "call_id":"toolu_1",
                    "output":"ok"
                }),
            ],
            model: Some("claude-sonnet-4-5".to_string()),
            reasoning_effort: None,
            instructions_override: None,
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: Some(vec![json!({
                "name": "lookup",
                "description": "Lookup",
                "input_schema": {"type":"object"}
            })]),
            tool_choice: None,
        };

        let payload = build_anthropic_payload(&resolved, &req).unwrap();
        assert_eq!(
            payload.get("model").and_then(Value::as_str),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(
            payload.get("max_tokens").and_then(Value::as_u64),
            Some(1024)
        );
        assert!(
            payload
                .get("system")
                .and_then(Value::as_str)
                .unwrap_or("")
                .contains("dev note")
        );
        let messages = payload.get("messages").and_then(Value::as_array).unwrap();
        assert_eq!(messages.len(), 3);
        assert!(
            messages[1]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("tool_use")
        );
        assert!(
            messages[2]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("tool_result")
        );
    }

    #[test]
    fn payload_normalizes_tool_choice_and_omits_openai_response_format() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![json!({
                "role": "user",
                "content": [{"type":"input_text","text":"reply as JSON"}]
            })],
            model: Some("claude-sonnet-4-5".to_string()),
            text: Some(json!({ "format": { "type": "json_object" } })),
            tools: Some(vec![json!({
                "name": "lookup",
                "description": "Lookup",
                "input_schema": {"type":"object"}
            })]),
            tool_choice: Some(Value::String("auto".to_string())),
            ..Default::default()
        };

        let payload = build_anthropic_payload(&resolved, &req).unwrap();
        assert_eq!(
            payload
                .get("tool_choice")
                .and_then(|choice| choice.get("type"))
                .and_then(Value::as_str),
            Some("auto")
        );
        assert!(payload.get("response_format").is_none());
    }

    #[test]
    fn payload_omits_tool_choice_when_no_tools_are_sent() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![json!({
                "role": "user",
                "content": [{"type":"input_text","text":"hello"}]
            })],
            model: Some("claude-sonnet-4-5".to_string()),
            tool_choice: Some(Value::String("auto".to_string())),
            ..Default::default()
        };

        let payload = build_anthropic_payload(&resolved, &req).unwrap();
        assert!(payload.get("tools").is_none());
        assert!(payload.get("tool_choice").is_none());
    }

    #[test]
    fn stream_parser_normalizes_tool_use_and_usage() {
        let mut response_id = String::new();
        let mut output_text = String::new();
        let mut output_items = Vec::new();
        let mut usage = None;
        let mut emitted_output_started = false;
        let mut id_factory = ProviderIdFactory::new("anthropic");
        let mut tool_blocks = BTreeMap::new();
        let mut events = Vec::new();

        let events_raw = [
            r#"data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"cache_read_input_tokens":2,"output_tokens":1}}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"agent\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"message_delta","usage":{"output_tokens":7}}"#,
        ];

        for raw in events_raw {
            process_anthropic_event(
                raw,
                &mut response_id,
                &mut output_text,
                &mut output_items,
                &mut usage,
                &mut emitted_output_started,
                &mut id_factory,
                &mut tool_blocks,
                &mut |event| {
                    events.push(event);
                    Ok(())
                },
            )
            .unwrap();
        }

        let usage = usage.unwrap();
        assert_eq!(response_id, "msg_1");
        assert_eq!(output_text, "hello");
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 19);
        assert!(output_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some("toolu_1")
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ProviderStreamEvent::ToolCallCompleted { call_id, .. } if call_id == "toolu_1"
            )
        }));
    }
}
