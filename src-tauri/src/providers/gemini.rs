use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Map, Value, json};
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

#[derive(Debug, Clone)]
struct GeminiToolCall {
    item_id: String,
    call_id: String,
    name: String,
    args: Value,
}

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let strategy = models_fetch_strategy(resolved);
    responses::models::fetch_remote_models_with_strategy(resolved, &strategy).await
}

fn models_fetch_strategy(resolved: &ResolvedModelConfig) -> responses::models::ModelsFetchStrategy {
    let mut strategy = responses::models::ModelsFetchStrategy::openai_compatible()
        .with_provider_overrides(&resolved.provider.models_endpoint_candidates);

    // Google Gemini's public REST API accepts API keys as a `key` query
    // parameter for both generation and model catalog endpoints.
    if should_use_key_query_param(&resolved.provider.api_endpoint) {
        strategy = strategy.with_credential_query_param("key");
    }

    strategy
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let mut id_factory = ProviderIdFactory::new("gemini");
    let credential = resolved.provider.resolved_credential();
    let endpoint = build_stream_endpoint(resolved, credential.as_deref())?;
    let headers = build_request_headers(resolved, credential.as_deref(), &endpoint);
    let body = build_gemini_payload(resolved, req)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request = responses::http::apply_headers_to_reqwest(
        client.post(endpoint.clone()).json(&body),
        &headers,
    )
    .map_err(|e| format!("Failed to prepare Gemini request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Gemini API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Gemini API error ({status}): {text}"));
    }

    let response_id = id_factory.response_id().to_string();
    let mut output_text = String::new();
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut emitted_output_started = false;
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
                let bytes = next_chunk.map_err(|e| format!("Failed to read Gemini stream: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    process_gemini_event(
                        &event_block,
                        &response_id,
                        &mut output_text,
                        &mut output_items,
                        &mut usage,
                        &mut emitted_output_started,
                        &mut id_factory,
                        on_delta,
                    )?;
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        process_gemini_event(
            &buffer,
            &response_id,
            &mut output_text,
            &mut output_items,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            on_delta,
        )?;
    }

    if !output_text.trim().is_empty()
        && !output_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
    {
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
        capabilities: registry::provider_capabilities("gemini")
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

fn build_stream_endpoint(
    resolved: &ResolvedModelConfig,
    credential: Option<&str>,
) -> Result<String, String> {
    let model = resolved.model_id.trim().trim_start_matches("models/");
    let base = resolved.provider.api_endpoint.trim_end_matches('/');
    let endpoint = format!("{base}/models/{model}:streamGenerateContent");
    let mut query_params = resolved.provider.query_params.clone();
    query_params
        .entry("alt".to_string())
        .or_insert_with(|| "sse".to_string());

    if should_use_key_query_param(base) {
        if let Some(credential) = credential.map(str::trim).filter(|value| !value.is_empty()) {
            query_params
                .entry("key".to_string())
                .or_insert_with(|| credential.to_string());
        }
    }

    responses::http::apply_query_params_to_url(&endpoint, &query_params)
        .map_err(|e| format!("Failed to build Gemini endpoint URL: {e}"))
}

fn should_use_key_query_param(endpoint: &str) -> bool {
    endpoint.contains("generativelanguage.googleapis.com")
}

fn build_request_headers(
    resolved: &ResolvedModelConfig,
    credential: Option<&str>,
    endpoint: &str,
) -> BTreeMap<String, String> {
    let bearer_credential = (!should_use_key_query_param(endpoint))
        .then_some(credential)
        .flatten();
    responses::http::merge_request_headers(
        &[("Content-Type", "application/json")],
        &resolved.provider,
        None,
        bearer_credential,
    )
}

fn build_gemini_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Value, String> {
    let mut payload = json!({
        "contents": build_gemini_contents(req)?,
    });

    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt);
    if !instructions.trim().is_empty() {
        payload["systemInstruction"] = json!({
            "parts": [{ "text": instructions }]
        });
    }

    if let Some(tools) = req.tools.as_ref().filter(|tools| !tools.is_empty()) {
        payload["tools"] = json!([{
            "functionDeclarations": tools
        }]);
    }

    let generation_config = build_generation_config(&resolved.request, req);
    if !generation_config.is_empty() {
        payload["generationConfig"] = Value::Object(generation_config);
    }

    apply_extra_body(&mut payload, &resolved.request);
    Ok(payload)
}

fn build_generation_config(
    request: &ModelRequestConfig,
    req: &ResponseStreamRequest,
) -> Map<String, Value> {
    let mut config = Map::new();
    if let Some(temperature) = request.temperature {
        config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        config.insert("topP".to_string(), json!(top_p));
    }
    if let Some(top_k) = request.top_k {
        config.insert("topK".to_string(), json!(top_k));
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        config.insert("maxOutputTokens".to_string(), json!(max_output_tokens));
    }
    if let Some(text) = &req.text {
        if let Some(format_type) = text
            .get("format")
            .and_then(|format| format.get("type"))
            .and_then(Value::as_str)
        {
            if format_type == "json_object" {
                config.insert(
                    "responseMimeType".to_string(),
                    Value::String("application/json".to_string()),
                );
            }
        }
    }
    config
}

fn apply_extra_body(payload: &mut Value, request: &ModelRequestConfig) {
    for (key, value) in &request.extra_body {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if matches!(
            normalized.as_str(),
            "contents" | "systeminstruction" | "tools" | "generationconfig"
        ) {
            continue;
        }
        payload[key] = value.clone();
    }
}

fn build_gemini_contents(req: &ResponseStreamRequest) -> Result<Vec<Value>, String> {
    let mut contents = Vec::new();
    let mut call_names_by_id = HashMap::new();

    for item in &req.input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "function_call" | "custom_tool_call" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                if !call_id.is_empty() && !name.is_empty() {
                    call_names_by_id.insert(call_id.to_string(), name.to_string());
                    contents.push(json!({
                        "role": "model",
                        "parts": [{
                            "functionCall": {
                                "name": name,
                                "args": parse_arguments_object(item.get("arguments"))
                            }
                        }]
                    }));
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let Some(name) = call_names_by_id.get(call_id) else {
                    continue;
                };
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": tool_output_response(item.get("output"))
                        }
                    }]
                }));
            }
            "reasoning" => {
                if let Some(text) = extract_reasoning_summary(item) {
                    contents.push(json!({
                        "role": "model",
                        "parts": [{ "text": text }]
                    }));
                }
            }
            _ => {
                if let Some(content) = gemini_content_from_message(item) {
                    contents.push(content);
                }
            }
        }
    }

    Ok(contents)
}

fn gemini_content_from_message(item: &Value) -> Option<Value> {
    let raw_role = item.get("role").and_then(Value::as_str)?.trim();
    let role = match raw_role {
        "assistant" => "model",
        "user" => "user",
        "system" | "developer" => "user",
        _ => "user",
    };
    let parts = gemini_parts_from_item(item)?;
    Some(json!({
        "role": role,
        "parts": parts
    }))
}

fn gemini_parts_from_item(item: &Value) -> Option<Vec<Value>> {
    let content = item.get("content")?;
    if let Some(text) = content.as_str() {
        return (!text.trim().is_empty()).then(|| vec![json!({ "text": text })]);
    }

    let parts = content.as_array()?;
    let mut gemini_parts = Vec::new();
    for part in parts {
        if let Some(text) = part
            .get("text")
            .or_else(|| part.get("input_text"))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                gemini_parts.push(json!({ "text": text }));
            }
            continue;
        }
        if let Some(image_part) = gemini_image_part(part) {
            gemini_parts.push(image_part);
        }
    }

    (!gemini_parts.is_empty()).then_some(gemini_parts)
}

fn gemini_image_part(part: &Value) -> Option<Value> {
    let url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })
        .or_else(|| part.get("image").and_then(Value::as_str))?;

    if let Some(data) = url.strip_prefix("data:") {
        let (mime_type, encoded) = data.split_once(";base64,")?;
        return Some(json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": encoded
            }
        }));
    }

    Some(json!({
        "fileData": {
            "mimeType": part
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("image/jpeg"),
            "fileUri": url
        }
    }))
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

fn tool_output_response(output: Option<&Value>) -> Value {
    let text = output.and_then(Value::as_str).unwrap_or("");
    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
        if parsed.is_object() {
            return parsed;
        }
    }
    json!({ "result": text })
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
fn process_gemini_event(
    block: &str,
    response_id: &str,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    emitted_output_started: &mut bool,
    id_factory: &mut ProviderIdFactory,
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
            "Failed to parse Gemini streaming event: {err}. body={}",
            preview(&payload)
        )
    })?;
    if let Some(error) = value.get("error") {
        return Err(format!("Gemini streaming error: {error}"));
    }
    if let Some(next_usage) = ProviderUsage::from_api_value(
        value
            .get("usageMetadata")
            .or_else(|| value.get("usage_metadata"))
            .unwrap_or(&value),
    ) {
        *usage = Some(next_usage);
    }

    let Some(candidates) = value.get("candidates").and_then(Value::as_array) else {
        return Ok(());
    };
    for candidate in candidates {
        let parts = candidate
            .get("content")
            .and_then(|content| content.get("parts"))
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();

        for part in parts {
            if let Some(text) = part.get("text").and_then(Value::as_str) {
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

            if let Some(function_call) = part.get("functionCall") {
                let tool_call = normalize_gemini_tool_call(function_call, id_factory)?;
                emit_gemini_tool_call(&tool_call, output_items, on_delta)?;
            }
        }
    }

    let _ = response_id;
    Ok(())
}

fn normalize_gemini_tool_call(
    function_call: &Value,
    id_factory: &mut ProviderIdFactory,
) -> Result<GeminiToolCall, String> {
    let name = function_call
        .get("name")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    if name.trim().is_empty() {
        return Err("Gemini functionCall is missing name".to_string());
    }
    let args = function_call
        .get("args")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let item_id = id_factory.next_item_id(&name);
    let call_id = id_factory.next_call_id(&name);

    Ok(GeminiToolCall {
        item_id,
        call_id,
        name,
        args,
    })
}

fn emit_gemini_tool_call(
    tool_call: &GeminiToolCall,
    output_items: &mut Vec<Value>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<(), String> {
    let arguments = serde_json::to_string(&tool_call.args)
        .map_err(|err| format!("Failed to serialize Gemini tool call args: {err}"))?;
    on_delta(ProviderStreamEvent::ToolCallStarted {
        item_id: tool_call.item_id.clone(),
        call_id: tool_call.call_id.clone(),
        name: tool_call.name.clone(),
        presentation: None,
    })?;
    on_delta(ProviderStreamEvent::ToolCallCompleted {
        item_id: tool_call.item_id.clone(),
        call_id: tool_call.call_id.clone(),
        name: tool_call.name.clone(),
        arguments: arguments.clone(),
        presentation: None,
    })?;
    output_items.push(json!({
        "type": "function_call",
        "id": tool_call.item_id,
        "call_id": tool_call.call_id,
        "name": tool_call.name,
        "arguments": arguments
    }));
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
    use super::{build_gemini_payload, models_fetch_strategy, process_gemini_event};
    use crate::config::{
        ModelRequestConfig, PromptComposerConfig, ProviderConfig, ResolvedModelConfig,
        compile_prompt_composer,
    };
    use crate::providers::core::ProviderIdFactory;
    use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest};
    use serde_json::{Value, json};

    fn test_resolved() -> ResolvedModelConfig {
        let prompt_assembly = compile_prompt_composer(&PromptComposerConfig::default());
        let provider = ProviderConfig {
            kind: "gemini".to_string(),
            api_endpoint: "https://generativelanguage.googleapis.com/v1beta".to_string(),
            ..Default::default()
        };
        ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "gemini".to_string(),
            provider,
            model_id: "gemini-2.5-flash".to_string(),
            model_ref: "gemini/gemini-2.5-flash".to_string(),
            system_prompt: "system prompt".to_string(),
            prompt_assembly,
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        }
    }

    #[test]
    fn models_strategy_uses_key_query_for_google_endpoint() {
        let resolved = test_resolved();
        let strategy = models_fetch_strategy(&resolved);

        assert_eq!(strategy.credential_query_param, Some("key"));
    }

    #[test]
    fn payload_converts_timeline_to_gemini_contents() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![
                json!({
                    "role": "user",
                    "content": [{"type":"input_text","text":"hello"}]
                }),
                json!({
                    "type":"function_call",
                    "call_id":"call_1",
                    "name":"lookup",
                    "arguments":"{\"q\":\"agent\"}"
                }),
                json!({
                    "type":"function_call_output",
                    "call_id":"call_1",
                    "output":"{\"ok\":true}"
                }),
            ],
            model: Some("gemini-2.5-flash".to_string()),
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
                "parameters": {"type":"object"}
            })]),
            tool_choice: None,
        };

        let payload = build_gemini_payload(&resolved, &req).unwrap();
        let contents = payload.get("contents").and_then(Value::as_array).unwrap();
        assert_eq!(contents.len(), 3);
        assert!(
            contents[1]
                .get("parts")
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|part| part.get("functionCall"))
                .is_some()
        );
        assert!(
            contents[2]
                .get("parts")
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|part| part.get("functionResponse"))
                .is_some()
        );
        assert!(
            payload
                .get("tools")
                .and_then(Value::as_array)
                .and_then(|tools| tools.first())
                .and_then(|tool| tool.get("functionDeclarations"))
                .is_some()
        );
    }

    #[test]
    fn stream_parser_normalizes_function_calls_and_usage() {
        let mut output_text = String::new();
        let mut output_items = Vec::new();
        let mut usage = None;
        let mut emitted_output_started = false;
        let mut id_factory = ProviderIdFactory::new("gemini");
        let mut events = Vec::new();
        let payload = r#"data: {"candidates":[{"content":{"parts":[{"text":"hello"},{"functionCall":{"name":"lookup","args":{"q":"agent"}}}]}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":8,"totalTokenCount":20}}"#;

        process_gemini_event(
            payload,
            "resp_1",
            &mut output_text,
            &mut output_items,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(output_text, "hello");
        assert_eq!(usage.unwrap().total_tokens, 20);
        assert!(output_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("name").and_then(Value::as_str) == Some("lookup")
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ProviderStreamEvent::ToolCallCompleted { name, .. } if name == "lookup"
            )
        }));
    }
}
