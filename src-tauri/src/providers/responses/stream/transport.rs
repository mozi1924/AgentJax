use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::sleep;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use crate::config::ResolvedModelConfig;
use crate::providers::types::{
    ProviderEventSink, ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};

use super::parser::{
    collect_output_item_from_sse_event_block, extract_output_items, extract_output_text,
    handle_stream_event_json, process_sse_event_block, split_sse_event_block, ParserState,
};
use super::{payload::build_streaming_request_payload, ResponsesStreamBehavior};
use crate::providers::responses::http;

fn resolved_stream_idle_timeout(resolved: &ResolvedModelConfig) -> Duration {
    let ms = resolved
        .provider
        .stream_idle_timeout_ms
        .unwrap_or_else(|| resolved.timeout_seconds.saturating_mul(1000))
        .max(1);
    Duration::from_millis(ms)
}

fn resolved_websocket_connect_timeout(resolved: &ResolvedModelConfig) -> Duration {
    let ms = resolved
        .provider
        .websocket_connect_timeout_ms
        .unwrap_or_else(|| resolved.timeout_seconds.saturating_mul(1000))
        .max(1);
    Duration::from_millis(ms)
}

fn should_retry_http_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn retry_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(4);
    let multiplier = 1u64 << shift;
    Duration::from_millis((150 * multiplier).min(2400))
}

pub(crate) async fn create_response_streaming_sse(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    behavior: ResponsesStreamBehavior,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential();
    let request_max_retries = resolved.provider.request_max_retries.unwrap_or(0);
    let idle_timeout = resolved_stream_idle_timeout(resolved);
    let request_headers = http::merge_request_headers(
        &[("Content-Type", "application/json")],
        &resolved.provider,
        None,
        credential.as_deref(),
    );

    let endpoint = format!(
        "{}/responses",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );
    let endpoint = http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params)
        .map_err(|e| format!("Failed to build SSE endpoint URL: {e}"))?;

    let body = build_streaming_request_payload(resolved, req, true);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let mut request_attempt = 0u32;
    let response = loop {
        let request = http::apply_headers_to_reqwest(
            client.post(endpoint.clone()).json(&body),
            &request_headers,
        )
        .map_err(|e| {
            format!(
                "Failed to prepare {} request headers: {e}",
                behavior.api_label
            )
        })?;

        let response = match request.send().await {
            Ok(response) => response,
            Err(err) => {
                if request_attempt < request_max_retries {
                    request_attempt += 1;
                    sleep(retry_delay(request_attempt)).await;
                    continue;
                }
                return Err(format!("Failed to reach {} API: {err}", behavior.api_label));
            }
        };

        if !response.status().is_success()
            && should_retry_http_status(response.status().as_u16())
            && request_attempt < request_max_retries
        {
            request_attempt += 1;
            sleep(retry_delay(request_attempt)).await;
            continue;
        }

        break response;
    };

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
    let mut accumulated_output_items: Vec<Value> = Vec::new();

    let mut buffer = String::new();
    let mut stream = response.bytes_stream();
    let mut cancelled = false;

    let state = Mutex::new(ParserState {
        emitted_reasoning_started: false,
        emitted_output_started: false,
        active_tools_map: HashMap::new(),
        assistant_message_phase_by_item: HashMap::new(),
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
          next_chunk = tokio::time::timeout(idle_timeout, stream.next()) => {
            let next_chunk = match next_chunk {
              Ok(next_chunk) => next_chunk,
              Err(_) => {
                return Err(format!(
                  "SSE stream idle timed out after {}ms",
                  idle_timeout.as_millis()
                ));
              }
            };
            let Some(next_chunk) = next_chunk else {
              break;
            };
            let bytes = next_chunk.map_err(|e| format!("Failed to read streaming response: {e}"))?;
            let chunk = String::from_utf8_lossy(&bytes);
            buffer.push_str(&chunk);

            while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
              buffer = rest;
              collect_output_item_from_sse_event_block(&event_block, &mut accumulated_output_items);
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
        collect_output_item_from_sse_event_block(&buffer, &mut accumulated_output_items);
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

    let output_items = if accumulated_output_items.is_empty() {
        last_response_obj
            .as_ref()
            .map(extract_output_items)
            .unwrap_or_default()
    } else {
        accumulated_output_items
    };

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
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential();
    let connect_timeout = resolved_websocket_connect_timeout(resolved);
    let idle_timeout = resolved_stream_idle_timeout(resolved);
    let request_headers =
        http::merge_request_headers(&[], &resolved.provider, None, credential.as_deref());

    let ws_url = format!(
        "{}/responses",
        resolved
            .provider
            .resolved_realtime_endpoint()
            .trim_end_matches('/')
    );
    let ws_url = http::apply_query_params_to_url(&ws_url, &resolved.provider.query_params)
        .map_err(|e| format!("Failed to build websocket endpoint URL: {e}"))?;

    let mut request = ws_url
        .clone()
        .into_client_request()
        .map_err(|e| format!("Failed to build websocket request: {e}"))?;
    http::apply_headers_to_websocket_request(&mut request, &request_headers)
        .map_err(|e| format!("Failed to apply websocket request headers: {e}"))?;

    let (mut ws, _) = tokio::time::timeout(connect_timeout, connect_async(request))
        .await
        .map_err(|_| {
            format!(
                "WebSocket connection timed out after {}ms",
                connect_timeout.as_millis()
            )
        })?
        .map_err(|e| format!("Failed to connect websocket transport: {e}"))?;

    let mut create_event = build_streaming_request_payload(
        resolved,
        req,
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
        assistant_message_phase_by_item: HashMap::new(),
        completed_tool_calls: Vec::new(),
    });

    loop {
        tokio::select! {
          changed = cancel_rx.changed() => {
            if changed.is_ok() && *cancel_rx.borrow() {
              break;
            }
          }
          next_message = tokio::time::timeout(idle_timeout, ws.next()) => {
            let next_message = match next_message {
              Ok(next_message) => next_message,
              Err(_) => {
                return Err(format!(
                  "WebSocket stream idle timed out after {}ms",
                  idle_timeout.as_millis()
                ));
              }
            };
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

    let _ = ws.close(None).await;

    if output_text.is_empty() {
        if let Some(obj) = &last_response_obj {
            output_text = extract_output_text(obj);
        }
    }

    let _ = on_delta(ProviderStreamEvent::ResponseCompleted);

    let output_items = if accumulated_output_items.is_empty() {
        last_response_obj
            .as_ref()
            .map(extract_output_items)
            .unwrap_or_default()
    } else {
        accumulated_output_items
    };

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
