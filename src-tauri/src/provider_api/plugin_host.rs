//! Provider plugin execution host.
//!
//! The host keeps only transport-neutral responsibilities: resolving the active
//! model profile, executing plugin callbacks, sending HTTP/SSE requests, and
//! forwarding normalized AgentJax stream events. Vendor-specific payloads,
//! headers, model catalog parsing, reasoning metadata, and stream event parsing
//! are supplied by the provider plugin itself.
//!
//! NOTE: This module is being phased out. New code should use the unified
//! `plugin_runtime::DenoCorePluginRuntime` which provides persistent JsRuntime
//! instances per plugin. For async contexts (streaming), this module still
//! creates temporary runtimes via `PluginInstance` from `runtime.rs`.

use std::collections::BTreeMap;
use std::str::FromStr;
use std::time::Duration;

use crate::error::{AgentJaxError, AgentJaxResult};
use futures_util::StreamExt;
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

use crate::config::{AppConfig, ResolvedModelConfig};
use crate::plugin_runtime::{PluginPackage, create_temp_plugin_instance};

use super::core::ProviderIdFactory;
use super::network::{apply_headers_to_reqwest, split_sse_event_block};
use super::registry;
use super::types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ProviderStreamEvent,
    ProviderUsage, ProviderUsageRecord, ResponseStreamRequest, ResponseStreamResult,
};

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginHttpRequest {
    url: String,
    #[serde(default = "default_http_method")]
    method: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    body: Option<Value>,
    #[serde(default = "default_stream_protocol")]
    stream_protocol: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PluginStreamStep {
    #[serde(default)]
    state: Value,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    events: Vec<ProviderStreamEvent>,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    output_text_delta: Option<String>,
    #[serde(default)]
    output_items: Vec<Value>,
    #[serde(default)]
    usage: Option<ProviderUsage>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
struct PluginStreamFinal {
    #[serde(default)]
    events: Vec<ProviderStreamEvent>,
    #[serde(default)]
    response_id: Option<String>,
    #[serde(default)]
    output_text: Option<String>,
    #[serde(default)]
    output_items: Vec<Value>,
    #[serde(default)]
    usage: Option<ProviderUsage>,
}

fn default_http_method() -> String {
    "POST".to_string()
}

fn default_stream_protocol() -> String {
    "sse".to_string()
}

pub async fn stream_response<F>(
    config: &AppConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    mut on_delta: F,
) -> AgentJaxResult<ResponseStreamResult>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    use crate::error::AgentJaxError;
    let resolved = config.resolve_model_profile(req.model.as_deref())?;
    let definition = registry::provider_definition(&resolved.provider.kind).ok_or_else(|| {
        AgentJaxError::config(format!(
            "Unsupported provider kind '{}'. Register a provider plugin to enable it.",
            resolved.provider.kind
        ))
    })?;
    let package = definition.plugin_package.clone().ok_or_else(|| {
        AgentJaxError::config(format!(
            "Provider kind '{}' is registered without an executable plugin package.",
            resolved.provider.kind
        ))
    })?;
    let context = plugin_context(&resolved, req);
    let request: PluginHttpRequest =
        call_provider_function(&package, &resolved.provider.kind, "buildStreamRequest", context)?;

    match request.stream_protocol.trim().to_ascii_lowercase().as_str() {
        "sse" => {
            stream_sse_request(
                &package,
                &resolved.provider.kind,
                request,
                &resolved,
                cancel_rx,
                &mut on_delta,
                definition.capabilities,
            )
            .await
            .map_err(Into::into)
        }
        "websocket" => {
            stream_websocket_request(
                &package,
                &resolved.provider.kind,
                request,
                &resolved,
                cancel_rx,
                &mut on_delta,
                definition.capabilities,
            )
            .await
            .map_err(Into::into)
        }
        other => Err(AgentJaxError::config(format!(
            "Provider plugin '{}' requested unsupported stream protocol '{}'. The host currently supports SSE and WebSocket for provider streams.",
            resolved.provider.kind, other
        ))),
    }
}

pub async fn fetch_remote_models(
    config: &AppConfig,
    provider_key: &str,
) -> AgentJaxResult<Vec<ProviderModelDescriptor>> {
    let provider = config.resolved_provider(provider_key)?;
    let prompt_assembly = config.compile_prompt_assembly();
    let resolved = ResolvedModelConfig {
        profile_key: "<catalog-sync>".to_string(),
        provider_key: provider_key.to_string(),
        model_ref: format!("{provider_key}/<catalog-sync>"),
        system_prompt: prompt_assembly.instructions_text.clone(),
        prompt_assembly,
        timeout_seconds: provider.resolved_timeout_seconds(config.request_timeout_seconds),
        provider,
        model_id: String::new(),
        request: Default::default(),
    };
    let package = registry::provider_plugin_package(&resolved.provider.kind).ok_or_else(|| {
        format!(
            "Provider kind '{}' is registered without an executable plugin package.",
            resolved.provider.kind
        )
    })?;
    let empty_request = ResponseStreamRequest::default();
    let request: PluginHttpRequest =
        call_provider_function(
            &package,
            &resolved.provider.kind,
            "buildModelsRequest",
            plugin_context(&resolved, &empty_request),
        )?;
    let response_json = send_json_request(&request, resolved.timeout_seconds).await?;
    call_provider_function(
        &package,
        &resolved.provider.kind,
        "parseModelsResponse",
        json!({
            "resolved": resolved_context(&resolved),
            "response": response_json
        }),
    )
}

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> AgentJaxResult<ModelReasoningCapability> {
    let package = registry::provider_plugin_package(provider_kind).ok_or_else(|| {
        format!(
            "Provider kind '{}' is registered without an executable plugin package.",
            provider_kind
        )
    })?;
    call_provider_function(
        &package,
        provider_kind,
        "getReasoningCapability",
        json!({
            "modelId": model_id,
            "cachedLevels": cached_levels.unwrap_or(&[])
        }),
    )
}

async fn stream_sse_request(
    package: &PluginPackage,
    provider_kind: &str,
    request: PluginHttpRequest,
    resolved: &ResolvedModelConfig,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
    capabilities: super::ProviderCapabilities,
) -> Result<ResponseStreamResult, String> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|err| format!("Failed to initialize provider HTTP client: {err}"))?;
    let method = Method::from_str(request.method.trim())
        .map_err(|err| format!("Invalid provider plugin HTTP method '{}': {err}", request.method))?;
    let mut builder = client.request(method, request.url.clone());
    if let Some(body) = &request.body {
        builder = builder.json(body);
    }
    let response = apply_headers_to_reqwest(builder, &request.headers)?
        .send()
        .await
        .map_err(|err| format!("Failed to reach provider stream endpoint: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Provider stream endpoint error ({status}): {text}"));
    }

    let mut state = json!({});
    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut stream_done = false;

    while !stream_done {
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
                    .map_err(|err| format!("Failed to read provider stream: {err}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    let done = apply_stream_step(
                        package,
                        provider_kind,
                        resolved,
                        event_block,
                        &mut state,
                        &mut response_id,
                        &mut output_text,
                        &mut output_items,
                        &mut usage,
                        on_delta,
                    )?;
                    if done {
                        stream_done = true;
                        break;
                    }
                }
            }
        }
    }

    if !stream_done && !buffer.trim().is_empty() {
        let _ = apply_stream_step(
            package,
            provider_kind,
            resolved,
            buffer,
            &mut state,
            &mut response_id,
            &mut output_text,
            &mut output_items,
            &mut usage,
            on_delta,
        )?;
    }

    let final_step: PluginStreamFinal = call_provider_function(
        package,
        provider_kind,
        "finalizeStream",
        json!({
            "resolved": resolved_context(resolved),
            "state": state
        }),
    )?;
    for event in final_step.events {
        on_delta(event)?;
    }
    if let Some(final_response_id) = final_step.response_id.filter(|value| !value.is_empty()) {
        response_id = final_response_id;
    }
    if let Some(final_text) = final_step.output_text {
        output_text = final_text;
    }
    output_items.extend(final_step.output_items);
    if final_step.usage.is_some() {
        usage = final_step.usage;
    }

    if response_id.is_empty() {
        response_id = ProviderIdFactory::new(&resolved.provider.kind).response_id().to_string();
    }
    let usage_hops = usage
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
        capabilities,
    })
}

async fn stream_websocket_request(
    package: &PluginPackage,
    provider_kind: &str,
    request: PluginHttpRequest,
    resolved: &ResolvedModelConfig,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
    capabilities: super::ProviderCapabilities,
) -> Result<ResponseStreamResult, String> {
    use futures_util::SinkExt;
    use tokio_tungstenite::connect_async;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;

    let mut ws_req = request.url.clone().into_client_request()
        .map_err(|err| format!("Invalid provider websocket URL '{}': {err}", request.url))?;

    // Apply headers from request
    for (key, value) in &request.headers {
        let key = key.trim();
        let value = value.trim();
        if !key.is_empty() && !value.is_empty() {
            if let Ok(header_name) = tokio_tungstenite::tungstenite::http::header::HeaderName::from_bytes(key.as_bytes()) {
                if let Ok(header_value) = tokio_tungstenite::tungstenite::http::header::HeaderValue::from_str(value) {
                    ws_req.headers_mut().insert(header_name, header_value);
                }
            }
        }
    }

    let (ws_stream, _) = connect_async(ws_req)
        .await
        .map_err(|err| format!("Failed to connect to provider websocket: {err}"))?;

    let (mut ws_tx, mut ws_rx) = ws_stream.split();

    // Send initial request body if present
    if let Some(body) = &request.body {
        let body_str = serde_json::to_string(body)
            .map_err(|err| format!("Failed to serialize websocket request body: {err}"))?;
        ws_tx.send(Message::text(body_str))
            .await
            .map_err(|err| format!("Failed to send websocket initial message: {err}"))?;
    }

    let mut state = json!({});
    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items = Vec::new();
    let mut usage = None;

    loop {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    break;
                }
            }
            msg = ws_rx.next() => {
                let Some(msg) = msg else {
                    break;
                };
                let msg = msg.map_err(|err| format!("Failed to read websocket message: {err}"))?;
                match msg {
                    Message::Text(text) => {
                        let done = apply_stream_step(
                            package,
                            provider_kind,
                            resolved,
                            text.to_string(),
                            &mut state,
                            &mut response_id,
                            &mut output_text,
                            &mut output_items,
                            &mut usage,
                            on_delta,
                        )?;
                        if done {
                            break;
                        }
                    }
                    Message::Close(_) => {
                        break;
                    }
                    _ => {}
                }
            }
        }
    }

    let final_step: PluginStreamFinal = call_provider_function(
        package,
        provider_kind,
        "finalizeStream",
        json!({
            "resolved": resolved_context(resolved),
            "state": state
        }),
    )?;
    for event in final_step.events {
        on_delta(event)?;
    }
    if let Some(final_response_id) = final_step.response_id.filter(|value| !value.is_empty()) {
        response_id = final_response_id;
    }
    if let Some(final_text) = final_step.output_text {
        output_text = final_text;
    }
    output_items.extend(final_step.output_items);
    if final_step.usage.is_some() {
        usage = final_step.usage;
    }

    if response_id.is_empty() {
        response_id = ProviderIdFactory::new(provider_kind).response_id().to_string();
    }
    let usage_hops = usage
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
        capabilities,
    })
}

#[allow(clippy::too_many_arguments)]
fn apply_stream_step(
    package: &PluginPackage,
    provider_kind: &str,
    resolved: &ResolvedModelConfig,
    event_block: String,
    state: &mut Value,
    response_id: &mut String,
    output_text: &mut String,
    output_items: &mut Vec<Value>,
    usage: &mut Option<ProviderUsage>,
    on_delta: &mut ProviderEventSink<'_>,
    ) -> Result<bool, String> {
    let step: PluginStreamStep = call_provider_function(
        package,
        provider_kind,
        "parseStreamEvent",
        json!({
            "resolved": resolved_context(resolved),
            "state": state,
            "eventBlock": event_block
        }),
    )?;
    *state = step.state;
    for event in step.events {
        on_delta(event)?;
    }
    if let Some(next_response_id) = step.response_id.filter(|value| !value.is_empty()) {
        *response_id = next_response_id;
    }
    if let Some(delta) = step.output_text_delta {
        output_text.push_str(&delta);
    }
    output_items.extend(step.output_items);
    if step.usage.is_some() {
        *usage = step.usage;
    }
    Ok(step.done)
}

async fn send_json_request(request: &PluginHttpRequest, timeout_seconds: u64) -> AgentJaxResult<Value> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|err| AgentJaxError::network(format!("Failed to initialize provider HTTP client: {err}")))?;
    let method = Method::from_str(request.method.trim())
        .map_err(|err| AgentJaxError::config(format!("Invalid provider plugin HTTP method '{}': {err}", request.method)))?;
    let mut builder = client.request(method, request.url.clone());
    if let Some(body) = &request.body {
        builder = builder.json(body);
    }
    let response = apply_headers_to_reqwest(builder, &request.headers)?
        .send()
        .await
        .map_err(|err| AgentJaxError::network(format!("Failed to reach provider endpoint: {err}")))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(AgentJaxError::internal(format!("Provider endpoint error ({status}): {text}")));
    }

    response
        .json()
        .await
        .map_err(|err| AgentJaxError::internal(format!("Failed to parse provider endpoint JSON: {err}")))
}

fn plugin_context(resolved: &ResolvedModelConfig, req: &ResponseStreamRequest) -> Value {
    json!({
        "resolved": resolved_context(resolved),
        "request": req,
    })
}

fn resolved_context(resolved: &ResolvedModelConfig) -> Value {
    json!({
        "providerKey": resolved.provider_key,
        "profileKey": resolved.profile_key,
        "modelId": resolved.model_id,
        "modelRef": resolved.model_ref,
        "systemPrompt": resolved.system_prompt,
        "requestConfig": resolved.request,
        "timeoutSeconds": resolved.timeout_seconds,
        "provider": resolved.provider,
        "credential": resolved.provider.resolved_credential(),
        "resolvedHttpHeaders": resolved.provider.resolved_http_headers(),
        "realtimeEndpoint": resolved.provider.resolved_realtime_endpoint(),
    })
}

/// Call a function on a provider plugin, creating a temporary runtime.
///
/// This is the legacy path used by `stream_response`, `fetch_remote_models`,
/// and `get_reasoning_capability` where the async HTTP context prevents using
/// a persistent `DenoCorePluginRuntime` (because `JsRuntime` is not `Send`).
///
/// Each call creates a fresh `PluginInstance` which evaluates the SDK bootstrap
/// and the plugin entrypoint. The JS function call itself is synchronous.
fn call_provider_function<T>(
    package: &PluginPackage,
    provider_kind: &str,
    function_name: &str,
    argument: Value,
) -> AgentJaxResult<T>
where
    T: for<'de> Deserialize<'de>,
{
    use crate::error::AgentJaxError;
    let mut instance = create_temp_plugin_instance(package)
        .map_err(|err| AgentJaxError::internal(format!("Failed to create plugin instance: {err}")))?;
    instance
        .call_provider_function::<T>(provider_kind, function_name, argument)
        .map_err(|err| AgentJaxError::tool(err.to_string()))
}
