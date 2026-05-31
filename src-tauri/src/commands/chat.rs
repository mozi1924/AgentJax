mod chat_client_metadata;
mod chat_events;
mod chat_persistence;
mod chat_prompt_tokens;
mod chat_registry;
mod chat_stream_observer;
mod chat_title;
mod chat_types;
mod chat_utils;

pub use chat_registry::ChatRequestRegistry;
pub use chat_types::{
    CancelChatRequest, ChatRequest, ChatResponse, DeleteConversationRequest,
    LoadConversationDynamicToolsRequest, LoadConversationRequest,
    RemoveConversationDynamicToolRequest, RenameConversationRequest,
    ReplaceConversationDynamicToolsRequest, UpsertConversationDynamicToolRequest,
};

use crate::config;
use crate::conversation_store;
use crate::provider_api::{build_user_input_item, get_tool_schema_format};
use crate::time_context::{build_temporal_context_developer_item, render_timed_message};
use crate::tools::ToolCatalog;
use crate::tools::ToolExecutionContext;
use chat_client_metadata::{split_local_client_metadata, validate_conversation_dynamic_tools};
use chat_events::{ChatStreamEvent, emit_mapped_stream_event, next_event_index};
use chat_prompt_tokens::{load_conversation_prompt_token_count, resolve_prompt_counting_model};
use chat_stream_observer::ChatStreamObserver;
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

    let tools_catalog = ToolCatalog::new_with_home_plugins(mcp_manager.inner().clone(), &config);

    let user_message_ts = now_unix_ms();

    {
        let conversation_id = conversation_id.clone();
        let request_id = request_id.clone();
        let input_text = input_text.clone();
        let user_message_ts = user_message_ts;
        run_blocking(move || {
            conversation_store::append_line(conversation_store::AppendLineInput {
                conversation_id,
                line: conversation_store::ConversationLine::User(conversation_store::UserLine {
                    id: format!("msg-user-{request_id}"),
                    ts: user_message_ts,
                    request_id,
                    text: input_text,
                }),
            })
        })
        .await?;
    }

    let resolved_model = resolve_prompt_counting_model(&config, req.model.as_deref());
    let model_id: Option<String> = resolved_model.as_ref().map(|m| m.model_id.clone());
    let context_token_count = if let Some(resolved_model) = resolved_model.as_ref() {
        let tool_context = ToolExecutionContext {
            conversation_id: Some(conversation_id.clone()),
        };
        let tool_schema_format = match get_tool_schema_format(&resolved_model.provider.kind) {
            Ok(format) => format,
            Err(err) => {
                log::warn!(
                    "Failed to resolve tool schema format for conversation '{}' while counting tokens: {}",
                    conversation_id,
                    err
                );
                crate::tools::ToolSchemaFormat::Responses
            }
        };
        let mounted_mcp_servers = tools_catalog.load_persisted_mounted_servers(&tool_context);
        let initial_snapshot = tools_catalog
            .snapshot_with_format_and_mounted_servers(
                tool_schema_format,
                &tool_context,
                &mounted_mcp_servers,
            )
            .await;
        let archived_context_items =
            crate::runtime::tool_archiving::archive_unavailable_historical_tool_calls(
                context.input_items.clone(),
                initial_snapshot.active_tool_names(),
            );
        let mut developer_items = resolved_model.prompt_assembly.developer_items.clone();
        developer_items.push(build_temporal_context_developer_item(
            now_unix_ms(),
            user_message_ts,
        ));
        let current_user_item = build_user_input_item(
            &resolved_model.provider.kind,
            &render_timed_message("Current user message", user_message_ts, &input_text),
        )?;

        match conversation_store::count_conversation_prompt_tokens(
            &resolved_model.model_id,
            Some(&resolved_model.system_prompt),
            &developer_items,
            recovery_note.as_ref(),
            &archived_context_items,
            &[current_user_item],
            initial_snapshot.schemas(),
        ) {
            Ok(usage) => usage.prompt_tokens,
            Err(err) => {
                log::warn!(
                    "Failed to count prompt tokens for conversation '{}' with model '{}': {}",
                    conversation_id,
                    resolved_model.model_id,
                    err
                );
                0
            }
        }
    } else {
        0
    };

    let closure_window = window.clone();
    let closure_request_id = request_id.clone();
    let closure_conversation_id = conversation_id.clone();

    let stream_observer = ChatStreamObserver::new(
        conversation_id.clone(),
        request_id.clone(),
        model_id,
        context_token_count,
    );
    let stream_observer_for_callback = stream_observer.clone();
    let mut runtime_req = req.clone();
    runtime_req.client_metadata = sanitized_client_metadata;
    let result = crate::runtime::AgentRuntime::run_turn(
        &config,
        &runtime_req,
        &conversation_id,
        user_message_ts,
        context.input_items,
        recovery_note,
        &tools_catalog,
        &mut cancel_rx,
        move |event| {
            let event_token_count = stream_observer_for_callback.handle_provider_event(&event);

            emit_mapped_stream_event(
                &closure_window,
                &closure_request_id,
                &closure_conversation_id,
                &mut event_index,
                event,
                event_token_count,
            )
        },
    )
    .await;

    let _ = registry.remove_chat_request(&request_id)?;
    let (response, _timeline_events) = result?;
    let final_token_count = stream_observer.persist_final_token_usage(&response);

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
                context_token_count: Some(final_token_count),
                error: None,
                tool_call_id: None,
                tool_name: None,
                tool_display_name: None,
                tool_description: None,
                tool_icon: None,
                tool_arguments: None,
                tool_output: None,
                tool_status: None,
                tool_started_ts: None,
                tool_completed_ts: None,
                tool_duration_ms: None,
                phase: None,
            },
        )
        .map_err(|e| format!("Failed to emit stream done event: {e}"))?;

    Ok(ChatResponse {
        response_id: response.response_id,
        output_text: response.output_text,
        conversation_id,
        conversation_title,
        context_token_count: final_token_count,
    })
}

#[tauri::command]
pub fn list_conversations() -> Result<Vec<conversation_store::ConversationSummary>, String> {
    conversation_store::list_conversations()
}

#[tauri::command]
pub async fn load_conversation(
    mcp_manager: State<'_, std::sync::Arc<crate::mcp::McpManager>>,
    req: LoadConversationRequest,
) -> Result<Option<conversation_store::ConversationDetail>, String> {
    let conversation_id = req.conversation_id.clone();
    let mut detail = conversation_store::load_conversation(&req.conversation_id)?;
    if let Some(detail_ref) = detail.as_mut() {
        detail_ref.context_token_count =
            match conversation_store::load_conversation_token_usage_count(&conversation_id)? {
                Some(count) => count,
                None => {
                    load_conversation_prompt_token_count(
                        &conversation_id,
                        req.model.as_deref(),
                        mcp_manager.inner().clone(),
                    )
                    .await
                }
            };
    }
    Ok(detail)
}

#[tauri::command]
pub fn load_conversation_dynamic_tools(
    req: LoadConversationDynamicToolsRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    conversation_store::load_conversation_dynamic_tools(&req.conversation_id)
}

#[tauri::command]
pub fn replace_conversation_dynamic_tools(
    req: ReplaceConversationDynamicToolsRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    validate_conversation_dynamic_tools(&req.tools)?;
    conversation_store::ensure_conversation(&req.conversation_id)?;
    conversation_store::update_conversation_dynamic_tools(&req.conversation_id, req.tools)?;
    conversation_store::load_conversation_dynamic_tools(&req.conversation_id)
}

#[tauri::command]
pub fn upsert_conversation_dynamic_tool(
    req: UpsertConversationDynamicToolRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    validate_conversation_dynamic_tools(std::slice::from_ref(&req.tool))?;
    conversation_store::ensure_conversation(&req.conversation_id)?;
    conversation_store::upsert_conversation_dynamic_tool(&req.conversation_id, req.tool)?;
    conversation_store::load_conversation_dynamic_tools(&req.conversation_id)
}

#[tauri::command]
pub fn remove_conversation_dynamic_tool(
    req: RemoveConversationDynamicToolRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    let tool_name = req.tool_name.trim();
    if tool_name.is_empty() {
        return Err("toolName cannot be empty".to_string());
    }
    conversation_store::ensure_conversation(&req.conversation_id)?;
    conversation_store::remove_conversation_dynamic_tool(&req.conversation_id, tool_name)?;
    conversation_store::load_conversation_dynamic_tools(&req.conversation_id)
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
    use super::{
        LoadConversationDynamicToolsRequest, RemoveConversationDynamicToolRequest,
        ReplaceConversationDynamicToolsRequest, UpsertConversationDynamicToolRequest,
        load_conversation_dynamic_tools, remove_conversation_dynamic_tool,
        replace_conversation_dynamic_tools, split_local_client_metadata,
        upsert_conversation_dynamic_tool, validate_conversation_dynamic_tools,
    };
    use crate::agentjax_home::AGENTJAX_HOME_ENV;
    use crate::config;
    use crate::conversation_store::ConversationDynamicTool;
    use crate::conversation_store::ConversationDynamicToolBinding;
    use serde_json::json;
    use uuid::Uuid;

    struct TestHomeGuard {
        home: std::path::PathBuf,
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var(AGENTJAX_HOME_ENV);
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    fn setup_test_home() -> TestHomeGuard {
        let home = std::env::temp_dir().join(format!("agentjax-chat-test-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&home).expect("create test home");
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }
        TestHomeGuard { home }
    }

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

    #[test]
    fn dynamic_tool_validation_rejects_invalid_name() {
        let err = validate_conversation_dynamic_tools(&[ConversationDynamicTool {
            name: "bad.name".to_string(),
            display_name: None,
            description: "Alias".to_string(),
            icon: None,
            parameters: json!({"type":"object","properties":{}}),
            binding: ConversationDynamicToolBinding::Native {
                tool: "calculator".to_string(),
            },
        }])
        .expect_err("invalid tool name should fail");
        assert!(err.contains("Dynamic tool name"));
    }

    #[test]
    fn dynamic_tool_commands_support_replace_upsert_and_remove() {
        let _guard = config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("conv-dtool-cmd-{}", Uuid::new_v4());

        let replaced = replace_conversation_dynamic_tools(ReplaceConversationDynamicToolsRequest {
            conversation_id: conversation_id.clone(),
            tools: vec![ConversationDynamicTool {
                name: "math_alias".to_string(),
                display_name: None,
                description: "Alias to calculator".to_string(),
                icon: None,
                parameters: json!({
                    "type": "object",
                    "properties": { "expression": { "type": "string" } }
                }),
                binding: ConversationDynamicToolBinding::Native {
                    tool: "calculator".to_string(),
                },
            }],
        })
        .expect("replace dynamic tools");
        assert_eq!(replaced.len(), 1);

        let upserted = upsert_conversation_dynamic_tool(UpsertConversationDynamicToolRequest {
            conversation_id: conversation_id.clone(),
            tool: ConversationDynamicTool {
                name: "time_alias".to_string(),
                display_name: None,
                description: "Alias to time tool".to_string(),
                icon: None,
                parameters: json!({
                    "type": "object",
                    "properties": {}
                }),
                binding: ConversationDynamicToolBinding::Native {
                    tool: "get_system_time".to_string(),
                },
            },
        })
        .expect("upsert dynamic tool");
        assert_eq!(upserted.len(), 2);

        let loaded = load_conversation_dynamic_tools(LoadConversationDynamicToolsRequest {
            conversation_id: conversation_id.clone(),
        })
        .expect("load dynamic tools");
        assert_eq!(loaded.len(), 2);

        let after_remove = remove_conversation_dynamic_tool(RemoveConversationDynamicToolRequest {
            conversation_id: conversation_id.clone(),
            tool_name: "math_alias".to_string(),
        })
        .expect("remove dynamic tool");
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].name, "time_alias");
    }
}
