use serde::{Deserialize, Serialize};
use std::collections::{HashMap, HashSet};
use std::sync::Mutex;
use tauri::{Emitter, Manager, State};
use tokio::sync::watch;

use crate::config;
use crate::conversation_store;
use crate::providers;
use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult};
use crate::tools::ToolCatalog;
use serde_json::Value;

const TITLE_GENERATION_INSTRUCTIONS: &str = "You generate concise conversation titles. Return only the title text with no quotes, no markdown, and no explanation. Match the user's language when it is obvious. Keep it under 12 Chinese characters or under 8 English words.";

#[derive(Debug, Clone)]
struct ActiveChatRequest {
    conversation_id: String,
    cancel_tx: watch::Sender<bool>,
}

#[derive(Debug, Clone)]
struct ActiveTitleRequest {
    job_id: String,
    cancel_tx: watch::Sender<bool>,
}

#[derive(Default)]
pub struct ChatRequestRegistry {
    requests: Mutex<HashMap<String, ActiveChatRequest>>,
    title_requests: Mutex<HashMap<String, ActiveTitleRequest>>,
    deleted_conversations: Mutex<HashSet<String>>,
}

impl ChatRequestRegistry {
    fn has_active_request_for_conversation(&self, conversation_id: &str) -> Result<bool, String> {
        let requests = self
            .requests
            .lock()
            .map_err(|_| "Failed to lock chat request registry".to_string())?;

        Ok(requests
            .values()
            .any(|request| request.conversation_id == conversation_id))
    }

    fn register_chat_request(
        &self,
        request_id: String,
        conversation_id: String,
        cancel_tx: watch::Sender<bool>,
    ) -> Result<(), String> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| "Failed to lock chat request registry".to_string())?;

        requests.insert(
            request_id,
            ActiveChatRequest {
                conversation_id,
                cancel_tx,
            },
        );
        Ok(())
    }

    fn remove_chat_request(&self, request_id: &str) -> Result<Option<ActiveChatRequest>, String> {
        let mut requests = self
            .requests
            .lock()
            .map_err(|_| "Failed to lock chat request registry".to_string())?;
        Ok(requests.remove(request_id))
    }

    fn cancel_chat_request(&self, request_id: &str) -> Result<bool, String> {
        let cancel_tx = {
            let requests = self
                .requests
                .lock()
                .map_err(|_| "Failed to lock chat request registry".to_string())?;
            requests
                .get(request_id)
                .map(|request| request.cancel_tx.clone())
        };

        if let Some(cancel_tx) = cancel_tx {
            cancel_tx
                .send(true)
                .map_err(|_| "Failed to signal chat stream cancellation".to_string())?;
            return Ok(true);
        }

        Ok(false)
    }

    fn register_title_request(
        &self,
        conversation_id: &str,
        cancel_tx: watch::Sender<bool>,
    ) -> Result<String, String> {
        let job_id = format!("title-{}-{}", conversation_id, chrono_like_now_id());
        let previous = {
            let mut title_requests = self
                .title_requests
                .lock()
                .map_err(|_| "Failed to lock title request registry".to_string())?;

            title_requests.insert(
                conversation_id.to_string(),
                ActiveTitleRequest {
                    job_id: job_id.clone(),
                    cancel_tx,
                },
            )
        };

        if let Some(previous) = previous {
            let _ = previous.cancel_tx.send(true);
        }

        Ok(job_id)
    }

    fn finish_title_request(&self, conversation_id: &str, job_id: &str) -> Result<(), String> {
        let mut title_requests = self
            .title_requests
            .lock()
            .map_err(|_| "Failed to lock title request registry".to_string())?;

        let should_remove = title_requests
            .get(conversation_id)
            .map(|request| request.job_id == job_id)
            .unwrap_or(false);

        if should_remove {
            title_requests.remove(conversation_id);
        }

        Ok(())
    }

    fn cancel_title_request(&self, conversation_id: &str) -> Result<bool, String> {
        let request = {
            let mut title_requests = self
                .title_requests
                .lock()
                .map_err(|_| "Failed to lock title request registry".to_string())?;
            title_requests.remove(conversation_id)
        };

        if let Some(request) = request {
            let _ = request.cancel_tx.send(true);
            return Ok(true);
        }

        Ok(false)
    }

    fn mark_conversation_deleted(&self, conversation_id: &str) -> Result<(), String> {
        let mut deleted_conversations = self
            .deleted_conversations
            .lock()
            .map_err(|_| "Failed to lock deleted conversation registry".to_string())?;
        deleted_conversations.insert(conversation_id.to_string());
        Ok(())
    }

    fn clear_conversation_deleted(&self, conversation_id: &str) -> Result<(), String> {
        let mut deleted_conversations = self
            .deleted_conversations
            .lock()
            .map_err(|_| "Failed to lock deleted conversation registry".to_string())?;
        deleted_conversations.remove(conversation_id);
        Ok(())
    }

    fn is_conversation_deleted(&self, conversation_id: &str) -> Result<bool, String> {
        let deleted_conversations = self
            .deleted_conversations
            .lock()
            .map_err(|_| "Failed to lock deleted conversation registry".to_string())?;
        Ok(deleted_conversations.contains(conversation_id))
    }

    fn cancel_conversation_tasks(&self, conversation_id: &str) -> Result<(), String> {
        let chat_cancel_txs = {
            let mut requests = self
                .requests
                .lock()
                .map_err(|_| "Failed to lock chat request registry".to_string())?;

            let request_ids = requests
                .iter()
                .filter_map(|(request_id, request)| {
                    if request.conversation_id == conversation_id {
                        Some(request_id.clone())
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>();

            let mut cancel_txs = Vec::with_capacity(request_ids.len());
            for request_id in request_ids {
                if let Some(request) = requests.remove(&request_id) {
                    cancel_txs.push(request.cancel_tx);
                }
            }

            cancel_txs
        };

        for cancel_tx in chat_cancel_txs {
            let _ = cancel_tx.send(true);
        }

        let _ = self.cancel_title_request(conversation_id)?;
        Ok(())
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub input: String,
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
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
    mcp_manager: State<'_, std::sync::Arc<crate::mcp::McpManager>>,
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

    if registry.has_active_request_for_conversation(&conversation_id)? {
        return Err("This conversation already has an active request. Stop it or wait for completion before sending another message.".to_string());
    }

    registry.clear_conversation_deleted(&conversation_id)?;

    let context = {
        let conversation_id = conversation_id.clone();
        let utility_model = utility_model.clone();
        run_blocking(move || {
            conversation_store::ensure_conversation(&conversation_id, &utility_model)?;
            conversation_store::load_context_for_request(&conversation_id)
        })
        .await?
    };

    registry.register_chat_request(request_id.clone(), conversation_id.clone(), cancel_tx)?;

    let tools_catalog = ToolCatalog::new(mcp_manager.inner().clone(), &config);

    let closure_window = window.clone();
    let closure_request_id = request_id.clone();
    let closure_conversation_id = conversation_id.clone();

    let result = crate::runtime::AgentRuntime::run_turn(
        &config,
        &req,
        &conversation_id,
        context.input_items,
        context.previous_response_id,
        &tools_catalog,
        &mut cancel_rx,
        move |event| {
            let mut chat_event = ChatStreamEvent {
                request_id: closure_request_id.clone(),
                event_index: next_event_index(&mut event_index),
                kind: "".to_string(),
                delta: None,
                response_id: None,
                conversation_id: Some(closure_conversation_id.clone()),
                conversation_title: None,
                error: None,
                tool_call_id: None,
                tool_name: None,
                tool_arguments: None,
                tool_output: None,
            };

            match event {
                ProviderStreamEvent::ReasoningStarted => {
                    chat_event.kind = "thinking".to_string();
                }
                ProviderStreamEvent::OutputTextStarted => {
                    chat_event.kind = "output_started".to_string();
                }
                ProviderStreamEvent::OutputTextDelta(delta) => {
                    chat_event.kind = "delta".to_string();
                    chat_event.delta = Some(delta);
                }
                ProviderStreamEvent::ToolCallStarted {
                    item_id: _,
                    call_id,
                    name,
                } => {
                    chat_event.kind = "tool_call_started".to_string();
                    chat_event.tool_call_id = Some(call_id);
                    chat_event.tool_name = Some(name);
                }
                ProviderStreamEvent::ToolCallArgumentsDelta {
                    item_id: _,
                    call_id,
                    delta,
                } => {
                    chat_event.kind = "tool_call_delta".to_string();
                    chat_event.tool_call_id = Some(call_id);
                    chat_event.delta = Some(delta);
                }
                ProviderStreamEvent::ToolCallCompleted {
                    item_id: _,
                    call_id,
                    name,
                    arguments,
                } => {
                    chat_event.kind = "tool_call_done".to_string();
                    chat_event.tool_call_id = Some(call_id);
                    chat_event.tool_name = Some(name);
                    chat_event.tool_arguments = Some(arguments);
                }
                ProviderStreamEvent::ToolCallExecuted { call_id, output } => {
                    chat_event.kind = "tool_call_exec".to_string();
                    chat_event.tool_call_id = Some(call_id);
                    chat_event.tool_output = Some(output);
                }
                ProviderStreamEvent::ResponseCompleted => {
                    return Ok(());
                }
            };

            closure_window
                .emit("chat_stream_event", chat_event)
                .map_err(|e| format!("Failed to emit stream event: {e}"))
        },
    )
    .await;

    let _ = registry.remove_chat_request(&request_id)?;
    let (response, timeline_events) = result?;

    let conversation_title: Option<String> = None;
    if !registry.is_conversation_deleted(&conversation_id)? {
        persist_completed_exchange(
            &conversation_id,
            &request_id,
            &input_text,
            &response,
            Some(timeline_events),
            &utility_model,
        )
        .await?;

        schedule_title_generation(
            window.clone(),
            window.app_handle().clone(),
            config.clone(),
            conversation_id.clone(),
            request_id.clone(),
        );
    }

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
                tool_call_id: None,
                tool_name: None,
                tool_arguments: None,
                tool_output: None,
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
    registry: State<'_, ChatRequestRegistry>,
    req: RenameConversationRequest,
) -> Result<conversation_store::ConversationSummary, String> {
    let config = config::load_config()?;
    let _ = registry.cancel_title_request(&req.conversation_id)?;

    conversation_store::rename_conversation(
        &req.conversation_id,
        &req.title,
        config.utility_small_model_key(),
    )
}

#[tauri::command]
pub fn delete_conversation(
    registry: State<'_, ChatRequestRegistry>,
    req: DeleteConversationRequest,
) -> Result<bool, String> {
    registry.mark_conversation_deleted(&req.conversation_id)?;
    registry.cancel_conversation_tasks(&req.conversation_id)?;

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
    tool_call_id: Option<String>,
    tool_name: Option<String>,
    tool_arguments: Option<String>,
    tool_output: Option<String>,
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
    registry.cancel_chat_request(&req.request_id)
}

fn schedule_title_generation(
    window: tauri::Window,
    app_handle: tauri::AppHandle,
    config: config::AppConfig,
    conversation_id: String,
    request_id: String,
) {
    tauri::async_runtime::spawn(async move {
        if let Err(err) =
            generate_title_and_emit(window, app_handle, config, &conversation_id, &request_id).await
        {
            log::warn!(
                "Failed to generate conversation title for {}: {}",
                conversation_id,
                err
            );
        }
    });
}

async fn generate_title_and_emit(
    window: tauri::Window,
    app_handle: tauri::AppHandle,
    config: config::AppConfig,
    conversation_id: &str,
    request_id: &str,
) -> Result<(), String> {
    let registry = app_handle.state::<ChatRequestRegistry>();
    if registry.is_conversation_deleted(conversation_id)? {
        return Ok(());
    }

    let candidate = {
        let conversation_id = conversation_id.to_string();
        run_blocking(move || conversation_store::load_title_generation_candidate(&conversation_id))
            .await?
    };

    let Some(candidate) = candidate else {
        return Ok(());
    };

    let (title_cancel_tx, mut title_cancel_rx) = watch::channel(false);
    let job_id = registry.register_title_request(conversation_id, title_cancel_tx)?;

    let title_request = ResponseStreamRequest {
        input_text: build_title_generation_prompt(&candidate),
        previous_response_id: None,
        model: Some(config.utility_small_model_key().to_string()),
        reasoning_effort: None,
        context_items: Vec::new(),
        instructions_override: Some(TITLE_GENERATION_INSTRUCTIONS.to_string()),
        tools: None,
        tool_choice: None,
    };

    let response =
        providers::stream_response(&config, &title_request, None, &mut title_cancel_rx, |_| {
            Ok(())
        })
        .await;

    let cancelled = *title_cancel_rx.borrow();
    registry.finish_title_request(conversation_id, &job_id)?;

    if cancelled || registry.is_conversation_deleted(conversation_id)? {
        return Ok(());
    }

    let response = response?;
    let title = sanitize_generated_title(&response.output_text);
    if title.is_empty() {
        return Ok(());
    }

    let updated_title = {
        let conversation_id = conversation_id.to_string();
        let title = title.clone();
        run_blocking(move || conversation_store::update_auto_title(&conversation_id, &title))
            .await?
    }
    .and_then(|summary| {
        let title = summary.title.trim().to_string();
        if title.is_empty() {
            None
        } else {
            Some(title)
        }
    });

    if let Some(conversation_title) = updated_title {
        window
            .emit(
                "chat_stream_event",
                ChatStreamEvent {
                    request_id: request_id.to_string(),
                    event_index: 0,
                    kind: "title".to_string(),
                    delta: None,
                    response_id: None,
                    conversation_id: Some(conversation_id.to_string()),
                    conversation_title: Some(conversation_title),
                    error: None,
                    tool_call_id: None,
                    tool_name: None,
                    tool_arguments: None,
                    tool_output: None,
                },
            )
            .map_err(|e| format!("Failed to emit title update event: {e}"))?;
    }

    Ok(())
}

async fn persist_completed_exchange(
    conversation_id: &str,
    request_id: &str,
    input_text: &str,
    response: &ResponseStreamResult,
    timeline_events: Option<Vec<Value>>,
    utility_model: &str,
) -> Result<(), String> {
    let conversation_id = conversation_id.to_string();
    let request_id = request_id.to_string();
    let input_text = input_text.to_string();
    let response = response.clone();
    let utility_model = utility_model.to_string();
    let now = now_unix_ms();

    run_blocking(move || {
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
                timeline_events: None,
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
                timeline_events: timeline_events,
                metadata: Default::default(),
            },
            &utility_model,
        )
    })
    .await
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

async fn run_blocking<T, F>(task: F) -> Result<T, String>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, String> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(task)
        .await
        .map_err(|err| format!("Background task join error: {err}"))?
}
