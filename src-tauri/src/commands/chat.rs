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
use chat_persistence::{persist_assistant_line, persist_tool_progress_event};
use chat_title::schedule_title_generation;
use chat_utils::{chrono_like_now_id, now_unix_ms, run_blocking};
use serde::Deserialize;
use serde_json::Value;
use tauri::{Emitter, Manager, State};
use tokio::sync::watch;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LocalClientMetadataEnvelope {
    #[serde(default)]
    dynamic_tools: Vec<conversation_store::ConversationDynamicTool>,
}

/// Extract AgentJax-local client metadata extensions and return a sanitized
/// payload safe to forward upstream.
fn split_local_client_metadata(
    client_metadata: Option<Value>,
) -> Result<
    (
        Option<Value>,
        Option<Vec<conversation_store::ConversationDynamicTool>>,
    ),
    String,
> {
    let Some(value) = client_metadata else {
        return Ok((None, None));
    };
    let Value::Object(mut metadata) = value else {
        return Ok((Some(value), None));
    };

    let Some(local_value) = metadata.remove("agentjax_local") else {
        return Ok((Some(Value::Object(metadata)), None));
    };

    let local: LocalClientMetadataEnvelope = serde_json::from_value(local_value)
        .map_err(|err| format!("Invalid agentjax_local client metadata: {err}"))?;
    let dynamic_tools = Some(local.dynamic_tools);

    let sanitized = if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    };
    Ok((sanitized, dynamic_tools))
}

#[tauri::command]
pub async fn chat_stream(
    window: tauri::Window,
    registry: State<'_, ChatRequestRegistry>,
    mcp_manager: State<'_, std::sync::Arc<crate::mcp::McpManager>>,
    req: ChatRequest,
) -> Result<ChatResponse, String> {
    let config = config::load_config()?;
    let (sanitized_client_metadata, local_dynamic_tools) =
        split_local_client_metadata(req.client_metadata.clone())?;
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
        let local_dynamic_tools = local_dynamic_tools.clone();
        run_blocking(move || {
            conversation_store::ensure_conversation(&conversation_id)?;
            if let Some(dynamic_tools) = local_dynamic_tools {
                conversation_store::update_conversation_dynamic_tools(
                    &conversation_id,
                    dynamic_tools,
                )?;
            }
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

    let recovery_note = {
        let conversation_id = conversation_id.clone();
        run_blocking(move || conversation_store::build_recovery_developer_note(&conversation_id))
            .await
            .ok()
            .flatten()
    };

    let tools_catalog = ToolCatalog::new(mcp_manager.inner().clone(), &config);

    {
        let conversation_id = conversation_id.clone();
        let request_id = request_id.clone();
        let input_text = input_text.clone();
        run_blocking(move || {
            conversation_store::append_line(conversation_store::AppendLineInput {
                conversation_id,
                line: conversation_store::ConversationLine::User(conversation_store::UserLine {
                    id: format!("msg-user-{request_id}"),
                    ts: now_unix_ms(),
                    request_id,
                    text: input_text,
                }),
            })
        })
        .await?;
    }

    let closure_window = window.clone();
    let closure_request_id = request_id.clone();
    let closure_conversation_id = conversation_id.clone();

    let callback_request_id = request_id.clone();
    let callback_conversation_id = conversation_id.clone();
    let mut runtime_req = req.clone();
    runtime_req.client_metadata = sanitized_client_metadata;
    let result = crate::runtime::AgentRuntime::run_turn(
        &config,
        &runtime_req,
        &conversation_id,
        context.input_items,
        recovery_note,
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
                        "tool_call_done",
                        call_id,
                        Some(name),
                        Some(arguments),
                    );
                }
                crate::providers::types::ProviderStreamEvent::ToolCallExecuted {
                    call_id,
                    name,
                    output,
                } => {
                    let _ = persist_tool_progress_event(
                        &callback_conversation_id,
                        &callback_request_id,
                        "tool_call_exec",
                        call_id,
                        Some(name),
                        Some(output),
                    );
                }
                crate::providers::types::ProviderStreamEvent::AssistantMessageCompleted {
                    text,
                    phase,
                    response_id,
                } => {
                    if *phase == Some(crate::message_phase::AssistantPhase::Commentary) {
                        let _ = persist_assistant_line(
                            &callback_conversation_id,
                            &callback_request_id,
                            response_id,
                            *phase,
                            text,
                        );
                    }
                }
                crate::providers::types::ProviderStreamEvent::HopAssistantText {
                    text,
                    phase,
                    response_id,
                } => {
                    if *phase != Some(crate::message_phase::AssistantPhase::Commentary) {
                        let _ = persist_assistant_line(
                            &callback_conversation_id,
                            &callback_request_id,
                            response_id,
                            *phase,
                            text,
                        );
                    }
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

    log::info!(
        "chat_stream turn complete: conv={} req={} text_len={} resp_id={} output_items={}",
        conversation_id,
        request_id,
        response.output_text.len(),
        response.response_id,
        response.output_items.len(),
    );

    let conversation_title: Option<String> = None;
    if !registry.is_conversation_deleted(&conversation_id)? {
        // Per-hop assistant lines were already persisted during streaming via
        // phase-aware HopAssistantText events.
        // No separate final persist needed.

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
                phase: None,
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
    let _ = registry.cancel_title_request(&req.conversation_id)?;

    conversation_store::rename_conversation(&req.conversation_id, &req.title)
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

#[cfg(test)]
mod tests {
    use super::split_local_client_metadata;
    use crate::conversation_store::ConversationDynamicToolBinding;
    use serde_json::json;

    #[test]
    fn local_dynamic_tools_are_extracted_and_removed_from_forwarded_metadata() {
        let (sanitized, dynamic_tools) = split_local_client_metadata(Some(json!({
            "trace_id": "abc",
            "agentjax_local": {
                "dynamicTools": [{
                    "name": "math_alias",
                    "description": "Alias",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "expression": { "type": "string" }
                        }
                    },
                    "binding": {
                        "type": "native",
                        "tool": "calculator"
                    }
                }]
            }
        })))
        .expect("split local metadata");

        assert_eq!(sanitized, Some(json!({ "trace_id": "abc" })));
        let dynamic_tools = dynamic_tools.expect("dynamic tools");
        assert_eq!(dynamic_tools.len(), 1);
        assert_eq!(dynamic_tools[0].name, "math_alias");
        assert_eq!(
            dynamic_tools[0].binding,
            ConversationDynamicToolBinding::Native {
                tool: "calculator".to_string()
            }
        );
    }
}
