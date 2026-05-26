mod chat_events;
mod chat_persistence;
mod chat_registry;
mod chat_title;
mod chat_types;
mod chat_utils;

pub use chat_registry::ChatRequestRegistry;
pub use chat_types::{
    CancelChatRequest, ChatRequest, ChatResponse, DeleteConversationRequest,
    LoadConversationRequest, RenameConversationRequest,
};

use crate::config;
use crate::conversation_store;
use crate::tools::ToolCatalog;
use chat_events::{emit_mapped_stream_event, next_event_index, ChatStreamEvent};
use chat_persistence::{persist_completed_exchange, persist_tool_progress_event};
use chat_title::schedule_title_generation;
use chat_utils::{chrono_like_now_id, now_unix_ms, run_blocking};
use tauri::{Emitter, Manager, State};
use tokio::sync::watch;

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

    if !registry.try_register_chat_request(
        request_id.clone(),
        conversation_id.clone(),
        cancel_tx.clone(),
    )? {
        return Err("This conversation already has an active request. Stop it or wait for completion before sending another message.".to_string());
    }

    registry.clear_conversation_deleted(&conversation_id)?;

    let context = match {
        let conversation_id = conversation_id.clone();
        let utility_model = utility_model.clone();
        run_blocking(move || {
            conversation_store::ensure_conversation(&conversation_id, &utility_model)?;
            conversation_store::load_context_for_request(&conversation_id)
        })
        .await
    } {
        Ok(context) => context,
        Err(err) => {
            let _ = registry.remove_chat_request(&request_id)?;
            return Err(err);
        }
    };

    let tools_catalog = ToolCatalog::new(mcp_manager.inner().clone(), &config);

    {
        let conversation_id = conversation_id.clone();
        let request_id = request_id.clone();
        let input_text = input_text.clone();
        let utility_model = utility_model.clone();
        let _ = run_blocking(move || {
            conversation_store::append_message(
                conversation_store::AppendMessageInput {
                    conversation_id,
                    entry_id: format!("msg-user-{request_id}"),
                    role: "user".to_string(),
                    text: input_text.clone(),
                    created_at_unix_ms: now_unix_ms(),
                    response_id: None,
                    provider: None,
                    model_profile: None,
                    model_id: None,
                    request_id: Some(request_id),
                    context_items: conversation_store::build_user_input_items(&input_text),
                    timeline_events: None,
                    metadata: Default::default(),
                },
                &utility_model,
            )
        })
        .await;
    }

    let closure_window = window.clone();
    let closure_request_id = request_id.clone();
    let closure_conversation_id = conversation_id.clone();

    let callback_utility_model = utility_model.clone();
    let callback_request_id = request_id.clone();
    let callback_conversation_id = conversation_id.clone();
    let result = crate::runtime::AgentRuntime::run_turn(
        &config,
        &req,
        &conversation_id,
        context.input_items,
        &tools_catalog,
        &mut cancel_rx,
        move |event| {
            match &event {
                crate::providers::types::ProviderStreamEvent::ToolCallCompleted {
                    call_id,
                    name,
                    arguments,
                    ..
                } => {
                    let _ = persist_tool_progress_event(
                        &callback_conversation_id,
                        &callback_request_id,
                        &callback_utility_model,
                        "tool_call_done",
                        call_id,
                        Some(name),
                        Some(arguments),
                    );
                }
                crate::providers::types::ProviderStreamEvent::ToolCallExecuted {
                    call_id,
                    output,
                } => {
                    let _ = persist_tool_progress_event(
                        &callback_conversation_id,
                        &callback_request_id,
                        &callback_utility_model,
                        "tool_call_exec",
                        call_id,
                        None,
                        Some(output),
                    );
                }
                _ => {}
            }

            emit_mapped_stream_event(
                &closure_window,
                &closure_request_id,
                &closure_conversation_id,
                &mut event_index,
                event,
            )
        },
    )
    .await;

    let _ = registry.remove_chat_request(&request_id)?;
    let (response, _timeline_events) = result?;

    let conversation_title: Option<String> = None;
    if !registry.is_conversation_deleted(&conversation_id)? {
        persist_completed_exchange(
            &conversation_id,
            &request_id,
            &input_text,
            &response,
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

#[tauri::command]
pub fn cancel_chat_stream(
    registry: State<'_, ChatRequestRegistry>,
    req: CancelChatRequest,
) -> Result<bool, String> {
    registry.cancel_chat_request(&req.request_id)
}
