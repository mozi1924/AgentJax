mod chat_events;
mod chat_persistence;
mod chat_registry;
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
use crate::providers::{build_user_input_item, get_tool_schema_format};
use crate::time_context::{build_temporal_context_developer_item, render_timed_message};
use crate::tools::ToolExecutionContext;
use crate::tools::{ToolCatalog, ToolCatalogSnapshot};
use chat_events::{ChatStreamEvent, emit_mapped_stream_event, next_event_index};
use chat_persistence::{persist_assistant_line, persist_tool_progress_event};
use chat_title::schedule_title_generation;
use chat_utils::{chrono_like_now_id, now_unix_ms, run_blocking};
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashSet;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
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
    validate_conversation_dynamic_tools(&local.dynamic_tools)?;
    let dynamic_tools = Some(local.dynamic_tools);

    let sanitized = if metadata.is_empty() {
        None
    } else {
        Some(Value::Object(metadata))
    };
    Ok((sanitized, dynamic_tools))
}

fn validate_dynamic_tool_name(name: &str) -> bool {
    let trimmed = name.trim();
    !trimmed.is_empty()
        && trimmed.len() <= 64
        && trimmed
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-')
}

/// Validate conversation-scoped dynamic tools before they are persisted.
///
/// Keeping the validation local and deterministic makes plugin-driven tool
/// registration easier to reason about and avoids storing malformed tool specs
/// that would later disappear from snapshots.
fn validate_conversation_dynamic_tools(
    tools: &[conversation_store::ConversationDynamicTool],
) -> Result<(), String> {
    let mut seen_names = HashSet::new();
    for tool in tools {
        if !validate_dynamic_tool_name(&tool.name) {
            return Err(format!(
                "Dynamic tool name '{}' must match [A-Za-z0-9_-] and be at most 64 characters",
                tool.name
            ));
        }
        if !seen_names.insert(tool.name.clone()) {
            return Err(format!("Duplicate dynamic tool name '{}'", tool.name));
        }
        if tool.description.trim().is_empty() {
            return Err(format!(
                "Dynamic tool '{}' must have a non-empty description",
                tool.name
            ));
        }
        if !tool.parameters.is_object() {
            return Err(format!(
                "Dynamic tool '{}' parameters must be a JSON object schema",
                tool.name
            ));
        }

        match &tool.binding {
            conversation_store::ConversationDynamicToolBinding::Native { tool: native_tool } => {
                if native_tool.trim().is_empty() {
                    return Err(format!(
                        "Dynamic tool '{}' has an empty native binding target",
                        tool.name
                    ));
                }
            }
            conversation_store::ConversationDynamicToolBinding::Mcp {
                server_id,
                tool: mcp_tool,
            } => {
                if server_id.trim().is_empty() || mcp_tool.trim().is_empty() {
                    return Err(format!(
                        "Dynamic tool '{}' must include non-empty MCP server_id and tool target",
                        tool.name
                    ));
                }
            }
        }
    }

    Ok(())
}

fn resolve_prompt_counting_model(
    config: &config::AppConfig,
    model: Option<&str>,
) -> Option<crate::config::ResolvedModelConfig> {
    match config.resolve_model_profile(model) {
        Ok(resolved) => Some(resolved),
        Err(err) => {
            log::warn!(
                "Failed to resolve prompt counting model from {:?}: {}",
                model,
                err
            );
            match config.resolve_model_profile(None) {
                Ok(resolved) => Some(resolved),
                Err(err) => {
                    log::warn!("Failed to resolve fallback prompt counting model: {}", err);
                    None
                }
            }
        }
    }
}

async fn tool_snapshot_for_conversation(
    tools_catalog: &ToolCatalog,
    conversation_id: &str,
    provider_kind: &str,
) -> Result<ToolCatalogSnapshot, String> {
    let tool_context = ToolExecutionContext {
        conversation_id: Some(conversation_id.to_string()),
    };
    let tool_schema_format = get_tool_schema_format(provider_kind)?;
    let mounted_mcp_servers = tools_catalog.load_persisted_mounted_servers(&tool_context);
    Ok(tools_catalog
        .snapshot_with_format_and_mounted_servers(
            tool_schema_format,
            &tool_context,
            &mounted_mcp_servers,
        )
        .await)
}

async fn load_conversation_prompt_token_count(
    conversation_id: &str,
    model: Option<&str>,
    mcp_manager: std::sync::Arc<crate::mcp::McpManager>,
) -> usize {
    let cfg = match config::load_config() {
        Ok(cfg) => cfg,
        Err(err) => {
            log::warn!("Failed to load config for token counting: {}", err);
            return 0;
        }
    };

    let Some(resolved_model) = resolve_prompt_counting_model(&cfg, model) else {
        return 0;
    };

    let tools_catalog = ToolCatalog::new(mcp_manager, &cfg);
    let tool_snapshot = match tool_snapshot_for_conversation(
        &tools_catalog,
        conversation_id,
        &resolved_model.provider.kind,
    )
    .await
    {
        Ok(snapshot) => snapshot,
        Err(err) => {
            log::warn!(
                "Failed to load tool snapshot for conversation '{}' while counting tokens: {}",
                conversation_id,
                err
            );
            return 0;
        }
    };

    let recovery_note = conversation_store::build_recovery_developer_note(conversation_id)
        .ok()
        .flatten();

    match conversation_store::load_context_for_request(conversation_id) {
        Ok(context) => {
            let archived_context_items =
                crate::runtime::tool_archiving::archive_unavailable_historical_tool_calls(
                    context.input_items,
                    tool_snapshot.active_tool_names(),
                );
            match conversation_store::count_conversation_prompt_tokens(
                &resolved_model.model_id,
                Some(&resolved_model.system_prompt),
                &resolved_model.prompt_assembly.developer_items,
                recovery_note.as_ref(),
                &archived_context_items,
                &[],
                tool_snapshot.schemas(),
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
        }
        Err(err) => {
            log::warn!(
                "Failed to load conversation '{}' for token counting: {}",
                conversation_id,
                err
            );
            0
        }
    }
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

    let callback_request_id = request_id.clone();
    let callback_conversation_id = conversation_id.clone();
    let fallback_token_count = Arc::new(AtomicUsize::new(context_token_count));
    let fallback_stream_count = Arc::clone(&fallback_token_count);
    let visible_token_count = Arc::new(AtomicUsize::new(context_token_count));
    let visible_stream_count = Arc::clone(&visible_token_count);
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
            match &event {
                crate::providers::types::ProviderStreamEvent::ToolCallCompleted {
                    call_id,
                    name,
                    arguments,
                    presentation,
                    ..
                } => {
                    let _ = persist_tool_progress_event(
                        &callback_conversation_id,
                        &callback_request_id,
                        "tool_call_done",
                        call_id,
                        Some(name),
                        presentation.as_ref().map(|meta| meta.display_name.as_str()),
                        presentation.as_ref().map(|meta| meta.description.as_str()),
                        presentation.as_ref().and_then(|meta| meta.icon.as_deref()),
                        Some(arguments),
                    );
                    // Count tokens for the persisted tool arguments and metadata
                    if let Some(ref mid) = model_id {
                        if let Ok(arg_tokens) =
                            conversation_store::count_text_tokens(mid, &arguments)
                        {
                            // Lightweight estimate for name / display_name / description
                            let meta_chars = name.len()
                                + presentation
                                    .as_ref()
                                    .map(|m| m.display_name.len())
                                    .unwrap_or(0)
                                + presentation
                                    .as_ref()
                                    .map(|m| m.description.len())
                                    .unwrap_or(0);
                            let meta_tokens = meta_chars.saturating_div(4);
                            fallback_stream_count.store(
                                fallback_stream_count
                                    .load(Ordering::Relaxed)
                                    .saturating_add(arg_tokens)
                                    .saturating_add(meta_tokens),
                                Ordering::Relaxed,
                            );
                        }
                    }
                }
                crate::providers::types::ProviderStreamEvent::ToolCallExecuted {
                    call_id,
                    name,
                    output,
                    presentation,
                    ..
                } => {
                    let _ = persist_tool_progress_event(
                        &callback_conversation_id,
                        &callback_request_id,
                        "tool_call_exec",
                        call_id,
                        Some(name),
                        presentation.as_ref().map(|meta| meta.display_name.as_str()),
                        presentation.as_ref().map(|meta| meta.description.as_str()),
                        presentation.as_ref().and_then(|meta| meta.icon.as_deref()),
                        Some(output),
                    );
                    // Count tokens for the tool result output
                    if let Some(ref mid) = model_id {
                        if let Ok(additional) = conversation_store::count_text_tokens(mid, output) {
                            fallback_stream_count.store(
                                fallback_stream_count
                                    .load(Ordering::Relaxed)
                                    .saturating_add(additional),
                                Ordering::Relaxed,
                            );
                        }
                    }
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
                        // Count tokens for persisted assistant commentary
                        if let Some(ref mid) = model_id {
                            if let Ok(additional) = conversation_store::count_text_tokens(mid, text)
                            {
                                fallback_stream_count.store(
                                    fallback_stream_count
                                        .load(Ordering::Relaxed)
                                        .saturating_add(additional),
                                    Ordering::Relaxed,
                                );
                            }
                        }
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
                        // Count tokens for persisted final answer text
                        if let Some(ref mid) = model_id {
                            if let Ok(additional) = conversation_store::count_text_tokens(mid, text)
                            {
                                fallback_stream_count.store(
                                    fallback_stream_count
                                        .load(Ordering::Relaxed)
                                        .saturating_add(additional),
                                    Ordering::Relaxed,
                                );
                            }
                        }
                    }
                }
                crate::providers::types::ProviderStreamEvent::UsageUpdated { usage, .. } => {
                    visible_stream_count.store(usage.total_tokens, Ordering::Relaxed);
                }
                _ => {}
            }

            let event_token_count = match &event {
                crate::providers::types::ProviderStreamEvent::UsageUpdated { usage, .. } => {
                    Some(usage.total_tokens)
                }
                _ => None,
            };

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
    if let Some(latest_usage_record) = response.usage_hops.last() {
        visible_token_count.store(latest_usage_record.usage.total_tokens, Ordering::Relaxed);
        if let Err(err) = conversation_store::update_conversation_token_usage(
            &conversation_id,
            &request_id,
            &latest_usage_record.response_id,
            "provider",
            "latest_response",
            &latest_usage_record.usage,
            response.usage.as_ref(),
            &response.usage_hops,
        ) {
            log::warn!(
                "Failed to persist provider token usage for conversation '{}': {}",
                conversation_id,
                err
            );
        }
    } else {
        let fallback_total = fallback_token_count.load(Ordering::Relaxed);
        visible_token_count.store(fallback_total, Ordering::Relaxed);
        let fallback_usage = crate::providers::types::ProviderUsage {
            prompt_tokens: fallback_total,
            completion_tokens: 0,
            total_tokens: fallback_total,
        };
        if let Err(err) = conversation_store::update_conversation_token_usage(
            &conversation_id,
            &request_id,
            &response.response_id,
            "local_estimate",
            "turn_estimate",
            &fallback_usage,
            None,
            &[],
        ) {
            log::warn!(
                "Failed to persist estimated token usage for conversation '{}': {}",
                conversation_id,
                err
            );
        }
    }
    let final_token_count = visible_token_count.load(Ordering::Relaxed);

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
