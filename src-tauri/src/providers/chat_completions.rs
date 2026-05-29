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
struct ChatToolCallAccumulator {
    item_id: String,
    call_id: String,
    name: String,
    arguments: String,
    started: bool,
    completed: bool,
}

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let strategy = responses::models::ModelsFetchStrategy::openai_compatible()
        .with_provider_overrides(&resolved.provider.models_endpoint_candidates);
    responses::models::fetch_remote_models_with_strategy(resolved, &strategy).await
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential();
    let request_headers = responses::http::merge_request_headers(
        &[("Content-Type", "application/json")],
        &resolved.provider,
        None,
        credential.as_deref(),
    );
    let endpoint = format!(
        "{}/chat/completions",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );
    let endpoint =
        responses::http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params)
            .map_err(|e| format!("Failed to build Chat Completions endpoint URL: {e}"))?;
    let body = build_chat_completions_payload(resolved, req)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request = responses::http::apply_headers_to_reqwest(
        client.post(endpoint).json(&body),
        &request_headers,
    )
    .map_err(|e| format!("Failed to prepare Chat Completions request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Chat Completions API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Chat Completions API error ({status}): {text}"));
    }

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut usage: Option<ProviderUsage> = None;
    let mut emitted_output_started = false;
    let mut id_factory = ProviderIdFactory::new("chat_completions");
    let mut tool_calls_by_index: BTreeMap<usize, ChatToolCallAccumulator> = BTreeMap::new();
    let mut output_items: Vec<Value> = Vec::new();
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
                let bytes = next_chunk
                    .map_err(|e| format!("Failed to read Chat Completions stream: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    process_chat_completions_event(
                        &event_block,
                        &mut response_id,
                        &mut output_text,
                        &mut usage,
                        &mut emitted_output_started,
                        &mut id_factory,
                        &mut tool_calls_by_index,
                        &mut output_items,
                        on_delta,
                    )?;
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        process_chat_completions_event(
            &buffer,
            &mut response_id,
            &mut output_text,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_calls_by_index,
            &mut output_items,
            on_delta,
        )?;
    }

    finalize_pending_tool_calls(&mut tool_calls_by_index, &mut output_items, on_delta)?;

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
        capabilities: registry::provider_capabilities("chat-completions")
            .unwrap_or_else(|| registry::default_provider_definition().capabilities),
    })
}

pub fn get_reasoning_capability(
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> ModelReasoningCapability {
    let supported_reasoning_levels = cached_levels
        .map(responses::normalize_reasoning_levels)
        .filter(|levels| !levels.is_empty())
        .unwrap_or_else(|| responses::infer_reasoning_levels_from_model_id(model_id));

    ModelReasoningCapability {
        supports_reasoning: !supported_reasoning_levels.is_empty(),
        supported_reasoning_levels,
    }
}

fn build_chat_completions_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Value, String> {
    let mut payload = json!({
        "model": resolved.model_id,
        "messages": build_chat_messages(resolved, req)?,
        "stream": true,
        "stream_options": {
            "include_usage": true
        }
    });

    apply_chat_model_request_config(
        &mut payload,
        &resolved.request,
        req.reasoning_effort.as_deref(),
    );

    if let Some(tools) = &req.tools {
        if !tools.is_empty() {
            payload["tools"] = json!(tools);
        }
    }
    if let Some(tool_choice) = &req.tool_choice {
        payload["tool_choice"] = tool_choice.clone();
    }
    if let Some(text) = &req.text {
        if let Some(format) = text.get("format") {
            payload["response_format"] = format.clone();
        }
    }
    if let Some(service_tier) = req
        .service_tier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload["service_tier"] = Value::String(service_tier.to_string());
    }
    if let Some(prompt_cache_key) = req
        .prompt_cache_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload["prompt_cache_key"] = Value::String(prompt_cache_key.to_string());
    }
    if let Some(client_metadata) = req
        .client_metadata
        .as_ref()
        .filter(|value| value.is_object())
    {
        payload["metadata"] = client_metadata.clone();
    }

    Ok(payload)
}

fn apply_chat_model_request_config(
    payload: &mut Value,
    request: &ModelRequestConfig,
    reasoning_effort_override: Option<&str>,
) {
    if let Some(temperature) = request.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        payload["top_p"] = json!(top_p);
    }
    if let Some(top_k) = request.top_k {
        payload["top_k"] = json!(top_k);
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        payload["max_tokens"] = json!(max_output_tokens);
    }
    if let Some(frequency_penalty) = request.frequency_penalty {
        payload["frequency_penalty"] = json!(frequency_penalty);
    }
    if let Some(presence_penalty) = request.presence_penalty {
        payload["presence_penalty"] = json!(presence_penalty);
    }
    if let Some(effort) = reasoning_effort_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            request
                .reasoning_effort
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
    {
        payload["reasoning_effort"] = json!(effort);
    }

    for (key, value) in &request.extra_body {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if matches!(
            normalized.as_str(),
            "model" | "messages" | "stream" | "stream_options"
        ) {
            continue;
        }

        payload[key] = value.clone();
    }
}

fn build_chat_messages(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt);
    if !instructions.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions
        }));
    }

    for item in &req.input_items {
        append_chat_message_from_input_item(item, &mut messages)?;
    }

    Ok(messages)
}

fn append_chat_message_from_input_item(
    item: &Value,
    messages: &mut Vec<Value>,
) -> Result<(), String> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    match item_type {
        "function_call" | "custom_tool_call" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            if call_id.trim().is_empty() || name.trim().is_empty() {
                return Ok(());
            }
            let arguments = stringify_arguments(item.get("arguments"))?;
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments
                    }
                }]
            }));
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if call_id.trim().is_empty() {
                return Ok(());
            }
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": item.get("output").and_then(Value::as_str).unwrap_or("")
            }));
        }
        "reasoning" => {
            if let Some(summary) = extract_reasoning_summary(item) {
                messages.push(json!({
                    "role": "assistant",
                    "content": summary
                }));
            }
        }
        _ => {
            if let Some(message) = build_role_message(item) {
                messages.push(message);
            }
        }
    }

    Ok(())
}

fn build_role_message(item: &Value) -> Option<Value> {
    let role = item.get("role").and_then(Value::as_str)?.trim();
    if role.is_empty() {
        return None;
    }
    let content = chat_content_from_item(item)?;
    Some(json!({
        "role": role,
        "content": content
    }))
}

fn chat_content_from_item(item: &Value) -> Option<Value> {
    let content = item.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(Value::String(text.to_string()));
    }

    let parts = content.as_array()?;
    let mut text = String::new();
    let mut chat_parts = Vec::new();
    for part in parts {
        if let Some(part_text) = part
            .get("text")
            .or_else(|| part.get("input_text"))
            .and_then(Value::as_str)
        {
            text.push_str(part_text);
            continue;
        }
        if let Some(image_part) = chat_image_part(part) {
            if !text.is_empty() {
                chat_parts.push(json!({
                    "type": "text",
                    "text": std::mem::take(&mut text)
                }));
            }
            chat_parts.push(image_part);
        }
    }

    if chat_parts.is_empty() {
        return (!text.trim().is_empty()).then_some(Value::String(text));
    }
    if !text.is_empty() {
        chat_parts.push(json!({
            "type": "text",
            "text": text
        }));
    }
    Some(Value::Array(chat_parts))
}

fn chat_image_part(part: &Value) -> Option<Value> {
    let image_url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .map(|url| json!({ "url": url }))
                .or_else(|| value.as_object().map(|_| value.clone()))
        })
        .or_else(|| {
            part.get("image")
                .and_then(Value::as_str)
                .map(|url| json!({ "url": url }))
        })?;

    Some(json!({
        "type": "image_url",
        "image_url": image_url
    }))
}

fn stringify_arguments(arguments: Option<&Value>) -> Result<String, String> {
    let Some(arguments) = arguments else {
        return Ok("{}".to_string());
    };
    if let Some(arguments) = arguments.as_str() {
        return Ok(arguments.to_string());
    }
    serde_json::to_string(arguments)
        .map_err(|err| format!("Failed to serialize Chat Completions tool arguments: {err}"))
}

fn extract_reasoning_summary(item: &Value) -> Option<String> {
    if let Some(summary) = item.get("summary") {
        if let Some(text) = summary.as_str() {
            return (!text.trim().is_empty()).then(|| text.trim().to_string());
        }
        if let Some(parts) = summary.as_array() {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            return (!text.trim().is_empty()).then(|| text.trim().to_string());
        }
    }
    None
}

#[allow(clippy::too_many_arguments)]
fn process_chat_completions_event(
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

fn finalize_pending_tool_calls(
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

#[cfg(test)]
mod tests {
    use super::{
        ChatToolCallAccumulator, build_chat_completions_payload, process_chat_completions_event,
    };
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
            provider_key: "chat-completions".to_string(),
            provider: ProviderConfig::default(),
            model_id: "gpt-4o".to_string(),
            model_ref: "chat-completions/gpt-4o".to_string(),
            system_prompt: "system prompt".to_string(),
            prompt_assembly,
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        }
    }

    #[test]
    fn payload_converts_responses_items_to_chat_messages() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![
                json!({
                    "role": "user",
                    "content": [{"type":"input_text","text":"hello"}]
                }),
                json!({
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": {"q": "agent"}
                }),
                json!({
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "ok"
                }),
            ],
            model: Some("gpt-4o".to_string()),
            reasoning_effort: None,
            instructions_override: None,
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: None,
            tool_choice: None,
        };

        let payload = build_chat_completions_payload(&resolved, &req).unwrap();
        let messages = payload.get("messages").and_then(Value::as_array).unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0].get("role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            messages[2]
                .get("tool_calls")
                .and_then(Value::as_array)
                .and_then(|calls| calls.first())
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            messages[3].get("tool_call_id").and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            payload
                .get("stream_options")
                .and_then(|v| v.get("include_usage"))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn stream_parser_normalizes_tool_calls_and_usage() {
        let mut response_id = String::new();
        let mut output_text = String::new();
        let mut usage = None;
        let mut emitted_output_started = false;
        let mut id_factory = ProviderIdFactory::new("chat_completions");
        let mut tool_calls_by_index: BTreeMap<usize, ChatToolCallAccumulator> = BTreeMap::new();
        let mut output_items = Vec::new();
        let mut events = Vec::new();

        let first = r#"data: {"id":"chatcmpl_1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\""}}]},"finish_reason":null}]}"#;
        let second = r#"data: {"id":"chatcmpl_1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"agent\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}}"#;

        process_chat_completions_event(
            first,
            &mut response_id,
            &mut output_text,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_calls_by_index,
            &mut output_items,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();
        process_chat_completions_event(
            second,
            &mut response_id,
            &mut output_text,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_calls_by_index,
            &mut output_items,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(response_id, "chatcmpl_1");
        assert_eq!(usage.unwrap().total_tokens, 14);
        assert_eq!(output_items.len(), 1);
        assert_eq!(
            output_items[0].get("call_id").and_then(Value::as_str),
            Some("call_1")
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ProviderStreamEvent::ToolCallCompleted { call_id, .. } if call_id == "call_1"
            )
        }));
    }
}
