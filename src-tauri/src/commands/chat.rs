use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Mutex;
use tauri::Emitter;
use tauri::State;
use tokio::sync::watch;

use crate::config;
use crate::conversation_store;
use crate::providers;
use crate::providers::types::ResponseStreamRequest;

#[derive(Default)]
pub struct ChatRequestRegistry {
    requests: Mutex<HashMap<String, watch::Sender<bool>>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub input: String,
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    pub request_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelChatRequest {
    pub request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub response_id: String,
    pub output_text: String,
    pub conversation_id: String,
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

    let input_text = req.input.trim().to_string();
    if input_text.is_empty() {
        return Err("Input text cannot be empty".to_string());
    }

    let conversation_id = req
        .conversation_id
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(conversation_store::new_conversation_id);

    let context = conversation_store::load_context_for_request(&conversation_id)?;

    registry
        .requests
        .lock()
        .map_err(|_| "Failed to lock chat request registry".to_string())?
        .insert(request_id.clone(), cancel_tx);

    let stream_request = ResponseStreamRequest {
        input_text: input_text.clone(),
        previous_response_id: context.previous_response_id,
        model: req.model.clone(),
        context_items: context.input_items,
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
                    response_id: None,
                    conversation_id: None,
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

    let now = now_unix_ms();
    conversation_store::append_message(conversation_store::AppendMessageInput {
        conversation_id: conversation_id.clone(),
        message_id: format!("msg-user-{request_id}"),
        role: "user".to_string(),
        text: input_text.clone(),
        created_at_unix_ms: now,
        response_id: None,
        provider: Some(response.provider_key.clone()),
        model_profile: Some(response.model_profile.clone()),
        model_id: Some(response.model_id.clone()),
        request_id: Some(request_id.clone()),
        input_items: conversation_store::build_user_input_items(&input_text),
        output_items: Vec::new(),
        metadata: Default::default(),
    })?;

    conversation_store::append_message(conversation_store::AppendMessageInput {
        conversation_id: conversation_id.clone(),
        message_id: format!("msg-assistant-{request_id}"),
        role: "assistant".to_string(),
        text: response.output_text.clone(),
        created_at_unix_ms: now_unix_ms(),
        response_id: Some(response.response_id.clone()),
        provider: Some(response.provider_key.clone()),
        model_profile: Some(response.model_profile.clone()),
        model_id: Some(response.model_id.clone()),
        request_id: Some(request_id.clone()),
        input_items: Vec::new(),
        output_items: response.output_items.clone(),
        metadata: Default::default(),
    })?;

    window
        .emit(
            "chat_stream_event",
            ChatStreamEvent {
                request_id: request_id.clone(),
                event_index: next_event_index(&mut event_index),
                kind: "done".to_string(),
                delta: Some(response.output_text.clone()),
                response_id: Some(response.response_id.clone()),
                conversation_id: Some(conversation_id.clone()),
                error: None,
            },
        )
        .map_err(|e| format!("Failed to emit stream done event: {e}"))?;

    Ok(ChatResponse {
        response_id: response.response_id,
        output_text: response.output_text,
        conversation_id,
    })
}

#[tauri::command]
pub fn list_conversations() -> Result<Vec<conversation_store::ConversationSummary>, String> {
    conversation_store::list_conversations()
}

#[tauri::command]
pub fn load_conversation(
    req: LoadConversationRequest,
) -> Result<Option<conversation_store::ConversationDetail>, String> {
    conversation_store::load_conversation(&req.conversation_id)
}

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct ChatStreamEvent {
    request_id: String,
    event_index: u64,
    kind: String,
    delta: Option<String>,
    response_id: Option<String>,
    conversation_id: Option<String>,
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

fn now_unix_ms() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
