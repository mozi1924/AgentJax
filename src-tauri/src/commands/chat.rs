use serde::{Deserialize, Serialize};
use tauri::Emitter;

use crate::config;
use crate::openai;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
  pub input: String,
  pub previous_response_id: Option<String>,
  pub model: Option<String>,
  pub request_id: Option<String>,
  pub history: Option<Vec<ChatHistoryMessage>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistoryMessage {
  pub role: String,
  pub text: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
  pub response_id: String,
  pub output_text: String,
}

#[tauri::command]
pub async fn chat_with_responses_stream(
  window: tauri::Window,
  req: ChatRequest,
) -> Result<ChatResponse, String> {
  let config = config::load_config()?;
  let request_id = req
    .request_id
    .clone()
    .unwrap_or_else(|| format!("req-{}", chrono_like_now_id()));
  let mut event_index: u64 = 0;

  let response = openai::create_response_streaming(
    &config,
    &req.input,
    req.history.as_deref(),
    req.model.as_deref(),
    req.previous_response_id.as_deref(),
    |delta| {
      window
        .emit(
          "chat_stream_event",
          ChatStreamEvent {
            request_id: request_id.clone(),
            event_index: next_event_index(&mut event_index),
            kind: "delta".to_string(),
            delta: Some(delta.to_string()),
            response_id: None,
            error: None,
          },
        )
        .map_err(|e| format!("Failed to emit stream delta: {e}"))
    },
  )
  .await?;

  window
    .emit(
      "chat_stream_event",
      ChatStreamEvent {
        request_id: request_id.clone(),
        event_index: next_event_index(&mut event_index),
        kind: "done".to_string(),
        delta: None,
        response_id: Some(response.id.clone()),
        error: None,
      },
    )
    .map_err(|e| format!("Failed to emit stream done event: {e}"))?;

  Ok(ChatResponse {
    response_id: response.id,
    output_text: response.output_text,
  })
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChatStreamEvent {
  request_id: String,
  event_index: u64,
  kind: String,
  delta: Option<String>,
  response_id: Option<String>,
  error: Option<String>,
}

fn next_event_index(current: &mut u64) -> u64 {
  *current += 1;
  *current
}

fn chrono_like_now_id() -> String {
  use std::time::{SystemTime, UNIX_EPOCH};
  let ts = SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_millis())
    .unwrap_or(0);
  ts.to_string()
}
