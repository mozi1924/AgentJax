use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::Emitter;
use tauri::State;
use tokio::sync::watch;

use crate::config;
use crate::providers;
use crate::providers::types::{ProviderMessage, ResponseStreamRequest};

#[derive(Default)]
pub struct ChatRequestRegistry {
    requests: Mutex<HashMap<String, watch::Sender<bool>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub input: String,
    pub continuation_id: Option<String>,
    pub model: Option<String>,
    pub request_id: Option<String>,
    pub history: Option<Vec<ChatHistoryMessage>>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatHistoryMessage {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelChatRequest {
    pub request_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub turn_id: String,
    pub output_text: String,
}

#[tauri::command]
pub async fn chat_stream(
    window: tauri::Window,
    registry: State<'_, ChatRequestRegistry>,
    req: ChatRequest,
) -> Result<ChatResponse, String> {
    let config = config::load_config()?;
    let request_id = req
        .request_id
        .clone()
        .unwrap_or_else(|| format!("req-{}", chrono_like_now_id()));
    let mut event_index: u64 = 0;
    let (cancel_tx, mut cancel_rx) = watch::channel(false);

    registry
        .requests
        .lock()
        .map_err(|_| "Failed to lock chat request registry".to_string())?
        .insert(request_id.clone(), cancel_tx);

    let stream_request = ResponseStreamRequest {
        input: req.input.clone(),
        continuation_id: req.continuation_id.clone(),
        model: req.model.clone(),
        history: req
            .history
            .clone()
            .unwrap_or_default()
            .into_iter()
            .map(|m| ProviderMessage {
                role: m.role,
                text: m.text,
            })
            .collect(),
    };

    let result = providers::stream_response(&config, &stream_request, &mut cancel_rx, |delta| {
        window
            .emit(
                "chat_stream_event",
                ChatStreamEvent {
                    request_id: request_id.clone(),
                    event_index: next_event_index(&mut event_index),
                    kind: "delta".to_string(),
                    delta: Some(delta.to_string()),
                    turn_id: None,
                    error: None,
                },
            )
            .map_err(|e| format!("Failed to emit stream delta: {e}"))
    })
    .await;

    registry
        .requests
        .lock()
        .map_err(|_| "Failed to lock chat request registry".to_string())?
        .remove(&request_id);

    let response = result?;

    window
        .emit(
            "chat_stream_event",
            ChatStreamEvent {
                request_id: request_id.clone(),
                event_index: next_event_index(&mut event_index),
                kind: "done".to_string(),
                delta: Some(response.output_text.clone()),
                turn_id: Some(response.turn_id.clone()),
                error: None,
            },
        )
        .map_err(|e| format!("Failed to emit stream done event: {e}"))?;

    Ok(ChatResponse {
        turn_id: response.turn_id,
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
    turn_id: Option<String>,
    error: Option<String>,
}

fn next_event_index(current: &mut u64) -> u64 {
    *current += 1;
    *current
}

#[tauri::command]
pub fn cancel_chat_stream(
    registry: State<'_, ChatRequestRegistry>,
    req: CancelChatRequest,
) -> Result<bool, String> {
    let requests = registry
        .requests
        .lock()
        .map_err(|_| "Failed to lock chat request registry".to_string())?;

    if let Some(cancel_tx) = requests.get(&req.request_id) {
        cancel_tx
            .send(true)
            .map_err(|_| "Failed to signal chat stream cancellation".to_string())?;
        return Ok(true);
    }

    Ok(false)
}

fn chrono_like_now_id() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    ts.to_string()
}
