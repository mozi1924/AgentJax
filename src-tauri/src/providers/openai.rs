use futures_util::{SinkExt, StreamExt};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use super::types::{
    ModelReasoningCapability, ProviderModelDescriptor, ProviderStreamEvent, ResponseStreamRequest,
    ResponseStreamResult,
};
use crate::config::{ModelRequestConfig, ResolvedModelConfig};

#[derive(Debug, Clone, Deserialize)]
struct RemoteModelsResponse {
    data: Vec<RemoteModel>,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteModel {
    id: String,
    #[serde(
        default,
        alias = "supportedReasoningLevels",
        alias = "supported_reasoning_levels"
    )]
    supported_reasoning_levels: Vec<String>,
}

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let credential = resolved.provider.resolved_credential().ok_or_else(|| {
        format!(
            "Provider '{}' credential is missing. Set credential in config.yaml or {} env.",
            resolved.provider_key, resolved.provider.credential_env
        )
    })?;

    let endpoint = format!(
        "{}/models",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );
    let response = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?
        .get(endpoint)
        .bearer_auth(credential)
        .send()
        .await
        .map_err(|e| format!("Failed to fetch remote models: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Failed to fetch remote models ({status}): {text}"));
    }

    let parsed: RemoteModelsResponse = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse remote model list: {e}"))?;

    let models = parsed
        .data
        .into_iter()
        .filter_map(|m| {
            let id = m.id.trim().to_string();
            if id.is_empty() {
                return None;
            }

            Some(ProviderModelDescriptor {
                id,
                supported_reasoning_levels: normalize_reasoning_levels(
                    &m.supported_reasoning_levels,
                ),
            })
        })
        .collect::<Vec<_>>();

    Ok(models)
}

pub async fn stream_response<F>(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    mut on_delta: F,
) -> Result<ResponseStreamResult, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String>,
{
    let persistence = if resolved.provider.stream_transport == "websocket" {
        false
    } else {
        resolved.provider.store_responses
    };
    let first_attempt = match resolved.provider.stream_transport.as_str() {
        "sse" => {
            create_response_streaming_sse(resolved, req, persistence, cancel_rx, &mut on_delta)
                .await
        }
        "websocket" => {
            create_response_streaming_websocket(
                resolved,
                req,
                persistence,
                cancel_rx,
                &mut on_delta,
            )
            .await
        }
        _ => {
            create_response_streaming_websocket(
                resolved,
                req,
                persistence,
                cancel_rx,
                &mut on_delta,
            )
            .await
        }
    };

    if should_retry_with_store_false(&first_attempt, persistence) {
        return match resolved.provider.stream_transport.as_str() {
            "sse" => {
                create_response_streaming_sse(resolved, req, false, cancel_rx, &mut on_delta).await
            }
            _ => {
                create_response_streaming_websocket(resolved, req, false, cancel_rx, &mut on_delta)
                    .await
            }
        };
    }

    if should_retry_without_previous_response(&first_attempt, req.previous_response_id.as_deref()) {
        let mut retry_req = req.clone();
        retry_req.previous_response_id = None;
        return match resolved.provider.stream_transport.as_str() {
            "sse" => {
                create_response_streaming_sse(
                    resolved,
                    &retry_req,
                    persistence,
                    cancel_rx,
                    &mut on_delta,
                )
                .await
            }
            _ => {
                create_response_streaming_websocket(
                    resolved,
                    &retry_req,
                    persistence,
                    cancel_rx,
                    &mut on_delta,
                )
                .await
            }
        };
    }

    first_attempt
}

fn should_retry_with_store_false(
    result: &Result<ResponseStreamResult, String>,
    store_value: bool,
) -> bool {
    if !store_value {
        return false;
    }

    let Err(err) = result else {
        return false;
    };

    err.contains("Store must be set to false")
        || err.contains("store must be set to false")
        || err.contains("\"Store must be set to false\"")
}

fn should_retry_without_previous_response(
    result: &Result<ResponseStreamResult, String>,
    previous_response_id: Option<&str>,
) -> bool {
    if previous_response_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_none()
    {
        return false;
    }

    let Err(err) = result else {
        return false;
    };

    err.contains("previous_response_not_found")
        || err.contains("Previous response with id")
        || err.contains("\"param\":\"previous_response_id\"")
}

fn build_streaming_request_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    previous_response_id: Option<&str>,
    store: bool,
) -> Value {
    let mut input_items = Vec::new();

    let previous_response_id = previous_response_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    if previous_response_id.is_none() {
        input_items.extend(req.context_items.iter().cloned());
    }

    input_items.push(json!({
      "role": "user",
      "content": [{
        "type": "input_text",
        "text": req.input_text
      }]
    }));

    let mut payload = json!({
      "model": resolved.model_id,
      "instructions": req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.provider.system_prompt),
      "input": input_items,
      "store": store,
      "stream": true
    });

    apply_model_request_config(
        &mut payload,
        &resolved.request,
        req.reasoning_effort.as_deref(),
    );

    if let Some(previous_id) = previous_response_id {
        payload["previous_response_id"] = Value::String(previous_id);
    }

    payload
}

fn apply_model_request_config(
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
        payload["max_output_tokens"] = json!(max_output_tokens);
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
        payload["reasoning"] = json!({ "effort": effort });
    }

    for (key, value) in &request.extra_body {
        if !key.trim().is_empty() {
            payload[key] = value.clone();
        }
    }
}

async fn create_response_streaming_sse<F>(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    store: bool,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut F,
) -> Result<ResponseStreamResult, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String>,
{
    let credential = resolved.provider.resolved_credential().ok_or_else(|| {
        format!(
            "Provider '{}' credential is missing. Set credential in config.yaml or {} env.",
            resolved.provider_key, resolved.provider.credential_env
        )
    })?;

    let endpoint = format!(
        "{}/responses",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );

    let body =
        build_streaming_request_payload(resolved, req, req.previous_response_id.as_deref(), store);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let response = client
        .post(endpoint)
        .bearer_auth(credential)
        .header("Content-Type", "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| format!("Failed to reach OpenAI API: {e}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("OpenAI API error ({status}): {text}"));
    }

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut last_response_obj: Option<Value> = None;

    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut cancelled = false;
    let mut emitted_reasoning_started = false;
    let mut emitted_output_started = false;

    loop {
        tokio::select! {
          changed = cancel_rx.changed() => {
            if changed.is_ok() && *cancel_rx.borrow() {
              cancelled = true;
              break;
            }
          }
          next_chunk = stream.next() => {
            let Some(next_chunk) = next_chunk else {
              break;
            };
            let bytes = next_chunk.map_err(|e| format!("Failed to read streaming response: {e}"))?;
            let chunk = String::from_utf8_lossy(&bytes);
            buffer.push_str(&chunk);

            while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
              buffer = rest;
              process_sse_event_block(
                &event_block,
                &mut response_id,
                &mut output_text,
                &mut last_response_obj,
                &mut emitted_reasoning_started,
                &mut emitted_output_started,
                on_delta,
              )?;
            }
          }
        }
    }

    if !buffer.trim().is_empty() {
        process_sse_event_block(
            &buffer,
            &mut response_id,
            &mut output_text,
            &mut last_response_obj,
            &mut emitted_reasoning_started,
            &mut emitted_output_started,
            on_delta,
        )?;
    }

    if output_text.is_empty() {
        if let Some(obj) = &last_response_obj {
            output_text = extract_output_text(obj);
        }
    }

    if cancelled && response_id.is_empty() {
        response_id = String::new();
    }

    let output_items = last_response_obj
        .as_ref()
        .map(extract_output_items)
        .unwrap_or_default();

    Ok(ResponseStreamResult {
        response_id,
        output_text,
        output_items,
        provider_key: resolved.provider_key.clone(),
        model_profile: resolved.profile_key.clone(),
        model_id: resolved.model_id.clone(),
    })
}

async fn create_response_streaming_websocket<F>(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    store: bool,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut F,
) -> Result<ResponseStreamResult, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String>,
{
    let credential = resolved.provider.resolved_credential().ok_or_else(|| {
        format!(
            "Provider '{}' credential is missing. Set credential in config.yaml or {} env.",
            resolved.provider_key, resolved.provider.credential_env
        )
    })?;

    let ws_url = format!(
        "{}/responses",
        resolved
            .provider
            .resolved_realtime_endpoint()
            .trim_end_matches('/')
    );

    let mut request = ws_url
        .clone()
        .into_client_request()
        .map_err(|e| format!("Failed to build websocket request: {e}"))?;
    request.headers_mut().insert(
        "Authorization",
        format!("Bearer {}", credential)
            .parse()
            .map_err(|e| format!("Failed to encode websocket authorization header: {e}"))?,
    );

    let (mut ws, _) = connect_async(request)
        .await
        .map_err(|e| format!("Failed to connect websocket transport: {e}"))?;

    let mut create_event =
        build_streaming_request_payload(resolved, req, req.previous_response_id.as_deref(), store);
    create_event["type"] = Value::String("response.create".to_string());

    ws.send(Message::Text(create_event.to_string().into()))
        .await
        .map_err(|e| format!("Failed to send websocket request: {e}"))?;

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut last_response_obj: Option<Value> = None;
    let mut emitted_reasoning_started = false;
    let mut emitted_output_started = false;

    loop {
        tokio::select! {
          changed = cancel_rx.changed() => {
            if changed.is_ok() && *cancel_rx.borrow() {
              break;
            }
          }
          next_message = ws.next() => {
            let Some(message) = next_message else {
              break;
            };
            let message = message.map_err(|e| format!("WebSocket receive error: {e}"))?;

            match message {
              Message::Text(text) => {
                handle_stream_event_json(
                  &text,
                  &mut response_id,
                  &mut output_text,
                  &mut last_response_obj,
                  &mut emitted_reasoning_started,
                  &mut emitted_output_started,
                  on_delta,
                )?;

                let maybe_done = serde_json::from_str::<Value>(&text)
                  .ok()
                  .and_then(|v| v.get("type").and_then(Value::as_str).map(ToOwned::to_owned));

                if matches!(
                  maybe_done.as_deref(),
                  Some("response.completed") | Some("response.done")
                ) {
                  break;
                }
              }
              Message::Binary(bin) => {
                if let Ok(text) = String::from_utf8(bin.to_vec()) {
                  handle_stream_event_json(
                    &text,
                    &mut response_id,
                    &mut output_text,
                    &mut last_response_obj,
                    &mut emitted_reasoning_started,
                    &mut emitted_output_started,
                    on_delta,
                  )?;
                }
              }
              Message::Close(_) => {
                break;
              }
              Message::Ping(payload) => {
                let _ = ws.send(Message::Pong(payload)).await;
              }
              Message::Pong(_) => {}
              Message::Frame(_) => {}
            }
          }
        }
    }

    let _ = ws.close(None).await;

    if output_text.is_empty() {
        if let Some(obj) = &last_response_obj {
            output_text = extract_output_text(obj);
        }
    }

    let output_items = last_response_obj
        .as_ref()
        .map(extract_output_items)
        .unwrap_or_default();

    Ok(ResponseStreamResult {
        response_id,
        output_text,
        output_items,
        provider_key: resolved.provider_key.clone(),
        model_profile: resolved.profile_key.clone(),
        model_id: resolved.model_id.clone(),
    })
}

fn split_sse_event_block(buffer: &str) -> Option<(String, String)> {
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

fn process_sse_event_block<F>(
    block: &str,
    response_id: &mut String,
    output_text: &mut String,
    last_response_obj: &mut Option<Value>,
    emitted_reasoning_started: &mut bool,
    emitted_output_started: &mut bool,
    on_delta: &mut F,
) -> Result<(), String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String>,
{
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
        emitted_reasoning_started,
        emitted_output_started,
        on_delta,
    )
}

fn handle_stream_event_json<F>(
    payload: &str,
    response_id: &mut String,
    output_text: &mut String,
    last_response_obj: &mut Option<Value>,
    emitted_reasoning_started: &mut bool,
    emitted_output_started: &mut bool,
    on_delta: &mut F,
) -> Result<(), String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String>,
{
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

    if !*emitted_reasoning_started
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
        *emitted_reasoning_started = true;
        on_delta(ProviderStreamEvent::ReasoningStarted)?;
    }

    if event_type == "response.output_text.delta" {
        if let Some(delta) = value.get("delta").and_then(Value::as_str) {
            if !*emitted_output_started {
                *emitted_output_started = true;
                on_delta(ProviderStreamEvent::OutputTextStarted)?;
            }
            output_text.push_str(delta);
            on_delta(ProviderStreamEvent::OutputTextDelta(delta.to_string()))?;
        }
    }

    if let Some(done_text) = value.get("text").and_then(Value::as_str) {
        if event_type == "response.output_text.done" && output_text.is_empty() {
            if !*emitted_output_started {
                *emitted_output_started = true;
                on_delta(ProviderStreamEvent::OutputTextStarted)?;
            }
            output_text.push_str(done_text);
        }
    }

    if let Some(response_obj) = value.get("response").and_then(Value::as_object) {
        *last_response_obj = Some(Value::Object(response_obj.clone()));
    }

    Ok(())
}

fn extract_output_items(root: &Value) -> Vec<Value> {
    root.get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn extract_output_text(root: &Value) -> String {
    if let Some(s) = root.get("output_text").and_then(Value::as_str) {
        return s.to_string();
    }

    if let Some(arr) = root.get("output_text").and_then(Value::as_array) {
        let joined = arr
            .iter()
            .filter_map(value_to_text)
            .collect::<Vec<_>>()
            .join("");
        if !joined.is_empty() {
            return joined;
        }
    }

    if let Some(output) = root.get("output").and_then(Value::as_array) {
        let mut chunks = Vec::new();
        for item in output {
            if let Some(content) = item.get("content").and_then(Value::as_array) {
                for c in content {
                    if let Some(text) = c.get("text").and_then(Value::as_str) {
                        chunks.push(text.to_string());
                    }
                }
            } else if let Some(text) = item.get("text").and_then(Value::as_str) {
                chunks.push(text.to_string());
            }
        }
        if !chunks.is_empty() {
            return chunks.join("");
        }
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

pub fn get_reasoning_capability(
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> ModelReasoningCapability {
    let supported_reasoning_levels = cached_levels
        .map(normalize_reasoning_levels)
        .filter(|levels| !levels.is_empty())
        .unwrap_or_else(|| fallback_reasoning_levels(model_id));

    ModelReasoningCapability {
        supports_reasoning: !supported_reasoning_levels.is_empty(),
        supported_reasoning_levels,
    }
}

fn normalize_reasoning_levels(levels: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();

    for level in levels {
        let level = level.trim().to_lowercase();
        if !matches!(
            level.as_str(),
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh"
        ) {
            continue;
        }

        if !normalized.iter().any(|existing| existing == &level) {
            normalized.push(level);
        }
    }

    normalized
}

fn fallback_reasoning_levels(model_id: &str) -> Vec<String> {
    let model_id = model_id.trim().to_lowercase();

    if model_id == "gpt-5" || model_id.starts_with("gpt-5-") {
        return vec![
            "minimal".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ];
    }

    if model_id.starts_with("gpt-5.1") {
        return vec![
            "none".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ];
    }

    if model_id.starts_with("gpt-5.2")
        || model_id.starts_with("gpt-5.5")
        || model_id.starts_with("gpt-5.3")
        || model_id.starts_with("gpt-5.4")
    {
        return vec![
            "none".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    Vec::new()
}
