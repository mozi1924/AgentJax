use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::watch;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::{client::IntoClientRequest, Message};

use crate::commands::chat::ChatHistoryMessage;
use crate::config::AppConfig;

pub struct ResponsesApiResponse {
  pub id: String,
  pub output_text: String,
}

pub async fn create_response_streaming<F>(
  config: &AppConfig,
  input: &str,
  history: Option<&[ChatHistoryMessage]>,
  requested_model: Option<&str>,
  previous_response_id: Option<&str>,
  cancel_rx: &mut watch::Receiver<bool>,
  mut on_delta: F,
) -> Result<ResponsesApiResponse, String>
where
  F: FnMut(&str) -> Result<(), String>,
{
  let store_value = if config.transport == "websocket" {
    false
  } else {
    config.store
  };
  let first_attempt = match config.transport.as_str() {
    "sse" => {
      create_response_streaming_sse(
        config,
        input,
        history,
        requested_model,
        previous_response_id,
        store_value,
        cancel_rx,
        &mut on_delta,
      )
        .await
    }
    "websocket" => {
      create_response_streaming_websocket(
        config,
        input,
        history,
        requested_model,
        previous_response_id,
        store_value,
        cancel_rx,
        &mut on_delta,
      )
      .await
    }
    _ => {
      create_response_streaming_websocket(
        config,
        input,
        history,
        requested_model,
        previous_response_id,
        store_value,
        cancel_rx,
        &mut on_delta,
      )
      .await
    }
  };

  if should_retry_with_store_false(&first_attempt, store_value) {
    return match config.transport.as_str() {
      "sse" => {
        create_response_streaming_sse(
          config,
          input,
          history,
          requested_model,
          previous_response_id,
          false,
          cancel_rx,
          &mut on_delta,
        )
        .await
      }
      _ => {
        create_response_streaming_websocket(
          config,
          input,
          history,
          requested_model,
          previous_response_id,
          false,
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
  result: &Result<ResponsesApiResponse, String>,
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

fn build_streaming_request_payload(
  config: &AppConfig,
  input: &str,
  history: Option<&[ChatHistoryMessage]>,
  model: &str,
  previous_response_id: Option<&str>,
  store: bool,
) -> Value {
  let mut input_items = Vec::new();
  if let Some(history_messages) = history {
    for message in history_messages {
      let role = message.role.trim().to_lowercase();
      if !matches!(role.as_str(), "user" | "assistant" | "system") {
        continue;
      }

      let text = message.text.trim();
      if text.is_empty() {
        continue;
      }

      input_items.push(json!({
        "role": role,
        "content": [{
          "type": "input_text",
          "text": text
        }]
      }));
    }
  }

  input_items.push(json!({
    "role": "user",
    "content": [{
      "type": "input_text",
      "text": input
    }]
  }));

  let mut payload = json!({
    "model": model,
    "instructions": config.instructions,
    "input": input_items,
    "store": store,
    "stream": true
  });

  if let Some(previous_id) = previous_response_id.map(str::trim).filter(|s| !s.is_empty()) {
    payload["previous_response_id"] = Value::String(previous_id.to_string());
  }

  payload
}

async fn create_response_streaming_sse<F>(
  config: &AppConfig,
  input: &str,
  history: Option<&[ChatHistoryMessage]>,
  requested_model: Option<&str>,
  previous_response_id: Option<&str>,
  store: bool,
  cancel_rx: &mut watch::Receiver<bool>,
  on_delta: &mut F,
) -> Result<ResponsesApiResponse, String>
where
  F: FnMut(&str) -> Result<(), String>,
{
  let api_key = config
    .resolved_api_key()
    .ok_or_else(|| "OPENAI API key is missing. Set api_key in config.yaml or OPENAI_API_KEY env.".to_string())?;

  let endpoint = format!("{}/responses", config.base_url.trim_end_matches('/'));
  let model = config.resolve_model(requested_model);

  let body = build_streaming_request_payload(
    config,
    input,
    history,
    &model,
    previous_response_id,
    store,
  );

  let client = reqwest::Client::builder()
    .timeout(Duration::from_secs(config.request_timeout_seconds))
    .build()
    .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

  let response = client
    .post(endpoint)
    .bearer_auth(api_key)
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

  Ok(ResponsesApiResponse {
    id: response_id,
    output_text,
  })
}

async fn create_response_streaming_websocket<F>(
  config: &AppConfig,
  input: &str,
  history: Option<&[ChatHistoryMessage]>,
  requested_model: Option<&str>,
  previous_response_id: Option<&str>,
  store: bool,
  cancel_rx: &mut watch::Receiver<bool>,
  on_delta: &mut F,
) -> Result<ResponsesApiResponse, String>
where
  F: FnMut(&str) -> Result<(), String>,
{
  let api_key = config
    .resolved_api_key()
    .ok_or_else(|| "OPENAI API key is missing. Set api_key in config.yaml or OPENAI_API_KEY env.".to_string())?;

  let ws_url = format!("{}/responses", config.resolved_websocket_url().trim_end_matches('/'));
  let model = config.resolve_model(requested_model);

  let mut request = ws_url
    .clone()
    .into_client_request()
    .map_err(|e| format!("Failed to build websocket request: {e}"))?;
  request.headers_mut().insert(
    "Authorization",
    format!("Bearer {}", api_key)
      .parse()
      .map_err(|e| format!("Failed to encode websocket authorization header: {e}"))?,
  );

  let (mut ws, _) = connect_async(request)
    .await
    .map_err(|e| format!("Failed to connect websocket transport: {e}"))?;

  let mut create_event = build_streaming_request_payload(
    config,
    input,
    history,
    &model,
    previous_response_id,
    store,
  );
  create_event["type"] = Value::String("response.create".to_string());

  ws.send(Message::Text(create_event.to_string()))
    .await
    .map_err(|e| format!("Failed to send websocket request: {e}"))?;

  let mut response_id = String::new();
  let mut output_text = String::new();
  let mut last_response_obj: Option<Value> = None;
  let mut cancelled = false;

  loop {
    tokio::select! {
      changed = cancel_rx.changed() => {
        if changed.is_ok() && *cancel_rx.borrow() {
          cancelled = true;
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

  if cancelled {
    let _ = ws.close(None).await;
  } else {
    let _ = ws.close(None).await;
  }

  if output_text.is_empty() {
    if let Some(obj) = &last_response_obj {
      output_text = extract_output_text(obj);
    }
  }

  Ok(ResponsesApiResponse {
    id: response_id,
    output_text,
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
  on_delta: &mut F,
) -> Result<(), String>
where
  F: FnMut(&str) -> Result<(), String>,
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

  handle_stream_event_json(&payload, response_id, output_text, last_response_obj, on_delta)
}

fn handle_stream_event_json<F>(
  payload: &str,
  response_id: &mut String,
  output_text: &mut String,
  last_response_obj: &mut Option<Value>,
  on_delta: &mut F,
) -> Result<(), String>
where
  F: FnMut(&str) -> Result<(), String>,
{
  let value: Value = serde_json::from_str(payload)
    .map_err(|e| format!("Failed to parse streaming event: {e}. body={}", preview(payload)))?;

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

  if event_type == "response.output_text.delta" {
    if let Some(delta) = value.get("delta").and_then(Value::as_str) {
      output_text.push_str(delta);
      on_delta(delta)?;
    }
  }

  if let Some(done_text) = value.get("text").and_then(Value::as_str) {
    if event_type == "response.output_text.done" && output_text.is_empty() {
      output_text.push_str(done_text);
    }
  }

  if let Some(response_obj) = value.get("response").and_then(Value::as_object) {
    *last_response_obj = Some(Value::Object(response_obj.clone()));
  }

  Ok(())
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
