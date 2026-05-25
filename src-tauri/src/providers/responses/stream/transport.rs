use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use crate::config::ResolvedModelConfig;
use crate::providers::types::{
    ProviderEventSink, ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};

use super::parser::{
    extract_output_items, extract_output_text, handle_stream_event_json, process_sse_event_block,
    split_sse_event_block, ParserState,
};
use super::{payload::build_streaming_request_payload, ResponsesStreamBehavior};

pub(crate) async fn create_response_streaming_sse(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    behavior: ResponsesStreamBehavior,
    store: bool,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential().ok_or_else(|| {
        format!(
            "Provider '{}' credential is missing.",
            resolved.provider_key
        )
    })?;

    let endpoint = format!(
        "{}/responses",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );

    let body = build_streaming_request_payload(
        resolved,
        req,
        req.previous_response_id.as_deref(),
        store,
        true,
    );

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
        .map_err(|e| format!("Failed to reach {} API: {e}", behavior.api_label))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!(
            "{} API error ({status}): {text}",
            behavior.api_label
        ));
    }

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut last_response_obj: Option<Value> = None;

    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut cancelled = false;

    let state = Mutex::new(ParserState {
        emitted_reasoning_started: false,
        emitted_output_started: false,
        active_tools_map: HashMap::new(),
        completed_tool_calls: Vec::new(),
    });

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
                &state,
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
            &state,
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
        capabilities: behavior.capabilities,
    })
}

pub(crate) async fn create_response_streaming_websocket(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    behavior: ResponsesStreamBehavior,
    store: bool,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential().ok_or_else(|| {
        format!(
            "Provider '{}' credential is missing.",
            resolved.provider_key
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

    let (mut ws, _) = tokio::time::timeout(
        Duration::from_secs(resolved.timeout_seconds),
        connect_async(request),
    )
    .await
    .map_err(|_| {
        format!(
            "WebSocket connection timed out after {}s",
            resolved.timeout_seconds
        )
    })?
    .map_err(|e| format!("Failed to connect websocket transport: {e}"))?;

    let mut create_event = build_streaming_request_payload(
        resolved,
        req,
        req.previous_response_id.as_deref(),
        store,
        behavior.capabilities.requires_stream_true_in_websocket,
    );
    create_event["type"] = Value::String("response.create".to_string());

    ws.send(Message::Text(create_event.to_string().into()))
        .await
        .map_err(|e| format!("Failed to send websocket request: {e}"))?;

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut last_response_obj: Option<Value> = None;
    let mut accumulated_output_items: Vec<Value> = Vec::new();

    let state = Mutex::new(ParserState {
        emitted_reasoning_started: false,
        emitted_output_started: false,
        active_tools_map: HashMap::new(),
        completed_tool_calls: Vec::new(),
    });

    let stream_result = tokio::time::timeout(
        Duration::from_secs(resolved.timeout_seconds),
        async {
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
                          &state,
                          on_delta,
                        )?;

                        let parsed_val: Value = serde_json::from_str(&text).unwrap_or_default();
                        let maybe_type = parsed_val.get("type").and_then(Value::as_str).unwrap_or("");

                        if maybe_type == "response.output_item.done" {
                            if let Some(item) = parsed_val.get("item") {
                                accumulated_output_items.push(item.clone());
                            }
                        }

                        if matches!(maybe_type, "response.completed" | "response.done") {
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
                            &state,
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
            Ok::<(), String>(())
        }
    )
    .await;

    match stream_result {
        Ok(inner) => inner?,
        Err(_) => {
            return Err(format!(
                "WebSocket stream timed out after {}s",
                resolved.timeout_seconds
            ))
        }
    }

    let _ = ws.close(None).await;

    if output_text.is_empty() {
        if let Some(obj) = &last_response_obj {
            output_text = extract_output_text(obj);
        }
    }

    let _ = on_delta(ProviderStreamEvent::ResponseCompleted);

    Ok(ResponseStreamResult {
        response_id,
        output_text,
        output_items: accumulated_output_items,
        provider_key: resolved.provider_key.clone(),
        model_profile: resolved.profile_key.clone(),
        model_id: resolved.model_id.clone(),
        capabilities: behavior.capabilities,
    })
}
