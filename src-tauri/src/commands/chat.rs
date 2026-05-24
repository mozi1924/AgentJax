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

const TITLE_GENERATION_INSTRUCTIONS: &str = "You generate concise conversation titles. Return only the title text with no quotes, no markdown, and no explanation. Match the user's language when it is obvious. Keep it under 12 Chinese characters or under 8 English words.";

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameConversationRequest {
    pub conversation_id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub response_id: String,
    pub output_text: String,
    pub conversation_id: String,
    pub conversation_title: Option<String>,
}

#[tauri::command]
pub async fn chat_stream(
    window: tauri::Window,
    registry: State<'_, ChatRequestRegistry>,
    req: ChatRequest,
) -> Result<ChatResponse, String> {
    let config = config::load_config()?;
    let utility_model = config.utility_small_model_key().to_string();
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

    conversation_store::ensure_conversation(&conversation_id, &utility_model)?;
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
        instructions_override: None,
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
                    conversation_title: None,
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
    conversation_store::append_message(
        conversation_store::AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: format!("msg-user-{request_id}"),
            role: "user".to_string(),
            text: input_text.clone(),
            created_at_unix_ms: now,
            response_id: None,
            provider: Some(response.provider_key.clone()),
            model_profile: Some(response.model_profile.clone()),
            model_id: Some(response.model_id.clone()),
            request_id: Some(request_id.clone()),
            context_items: conversation_store::build_user_input_items(&input_text),
            metadata: Default::default(),
        },
        &utility_model,
    )?;

    conversation_store::append_message(
        conversation_store::AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: format!("msg-assistant-{request_id}"),
            role: "assistant".to_string(),
            text: response.output_text.clone(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some(response.response_id.clone()),
            provider: Some(response.provider_key.clone()),
            model_profile: Some(response.model_profile.clone()),
            model_id: Some(response.model_id.clone()),
            request_id: Some(request_id.clone()),
            context_items: response.output_items.clone(),
            metadata: Default::default(),
        },
        &utility_model,
    )?;

    let conversation_title = generate_title_if_needed(&config, &conversation_id)
        .await
        .map_err(|err| {
            log::warn!(
                "Failed to generate conversation title for {}: {}",
                conversation_id,
                err
            );
            err
        })
        .ok()
        .flatten();

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
                conversation_title: conversation_title.clone(),
                error: None,
            },
        )
        .map_err(|e| format!("Failed to emit stream done event: {e}"))?;

    Ok(ChatResponse {
        response_id: response.response_id,
        output_text: response.output_text,
        conversation_id,
        conversation_title,
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

#[tauri::command]
pub fn rename_conversation(
    req: RenameConversationRequest,
) -> Result<conversation_store::ConversationSummary, String> {
    let config = config::load_config()?;
    conversation_store::rename_conversation(
        &req.conversation_id,
        &req.title,
        config.utility_small_model_key(),
    )
}

#[tauri::command]
pub fn delete_conversation(req: DeleteConversationRequest) -> Result<bool, String> {
    conversation_store::delete_conversation(&req.conversation_id)
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
    conversation_title: Option<String>,
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

async fn generate_title_if_needed(
    config: &config::AppConfig,
    conversation_id: &str,
) -> Result<Option<String>, String> {
    let Some(candidate) = conversation_store::load_title_generation_candidate(conversation_id)?
    else {
        return Ok(None);
    };

    let mut title_cancel_rx = watch::channel(false).1;
    let title_request = ResponseStreamRequest {
        input_text: build_title_generation_prompt(&candidate),
        previous_response_id: None,
        model: Some(config.utility_small_model_key().to_string()),
        context_items: Vec::new(),
        instructions_override: Some(TITLE_GENERATION_INSTRUCTIONS.to_string()),
    };

    let response =
        providers::stream_response(config, &title_request, &mut title_cancel_rx, |_| Ok(()))
            .await?;

    let title = sanitize_generated_title(&response.output_text);
    if title.is_empty() {
        return Ok(None);
    }

    let updated = conversation_store::update_auto_title(conversation_id, &title)?;
    Ok(updated.map(|summary| summary.title))
}

fn build_title_generation_prompt(
    candidate: &conversation_store::TitleGenerationCandidate,
) -> String {
    format!(
        "User message:\n{}\n\nAssistant reply:\n{}\n\nGenerate one concise conversation title.",
        candidate.user_text.trim(),
        candidate.assistant_text.trim()
    )
}

fn sanitize_generated_title(raw: &str) -> String {
    let first_line = raw
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("");

    let cleaned = first_line
        .trim_matches('"')
        .trim_matches('“')
        .trim_matches('”')
        .trim_matches('`')
        .trim();

    if cleaned.is_empty() {
        return String::new();
    }

    let compact = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.chars().count() <= 32 {
        compact
    } else {
        compact.chars().take(32).collect()
    }
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
