mod chat_client_metadata;
pub(crate) mod chat_events;
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
use crate::time_context::{build_temporal_context_system_item, render_timed_message};
use crate::runtime::agent_context::{AgentContext, LcmAgentContext};
use crate::tools::ToolCatalog;
use crate::tools::ToolExecutionContext;
use chat_client_metadata::{split_local_client_metadata, validate_conversation_dynamic_tools};
use chat_events::{ChatStreamEvent, emit_mapped_stream_event, next_event_index};
use chat_prompt_tokens::{load_conversation_prompt_token_count, resolve_prompt_counting_model};
use chat_stream_observer::ChatStreamObserver;
use chat_title::schedule_title_generation;
use chat_utils::{chrono_like_now_id, now_unix_ms, run_blocking};
use serde_json::Value;
use std::sync::Arc;
use tauri::{Emitter, Manager, State};
use tokio::sync::watch;

#[tauri::command]
pub async fn chat_stream(
    window: tauri::Window,
    registry: State<'_, ChatRequestRegistry>,
    mcp_manager: State<'_, std::sync::Arc<crate::mcp::McpManager>>,
    req: ChatRequest,
) -> Result<ChatResponse, String> {
    let agent_id = req
        .agent_id
        .as_deref()
        .unwrap_or(crate::config::constants::DEFAULT_AGENT_ID)
        .to_string();

    // Load FullConfig: shared providers/config + agent-specific settings
    let full_config = config::load_full_config(&agent_id)?;
    let config = full_config.shared.clone();
    let agent_config = full_config.agent.clone();
    let jsonl_backup_enabled = agent_config.context_management.jsonl_backup_enabled;
    let (sanitized_client_metadata, local_dynamic_tools) =
        split_local_client_metadata(req.client_metadata.clone())?;
    let request_id = req
        .request_id
        .clone()
        .unwrap_or_else(|| format!("req-{}", chrono_like_now_id()));
    let event_index = Arc::new(std::sync::atomic::AtomicU64::new(0));
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

    let _closure_agent_id = agent_id.clone();
    let mut context = {
        let conversation_id = conversation_id.clone();
        let local_dynamic_tools = local_dynamic_tools.clone();
        let agent_id = agent_id.clone();
        run_blocking(move || {
            conversation_store::ensure_conversation(&agent_id, &conversation_id)?;
            if let Some(dynamic_tools) = local_dynamic_tools {
                conversation_store::update_conversation_dynamic_tools(
                    &agent_id,
                    &conversation_id,
                    dynamic_tools,
                )?;
            }
            conversation_store::load_context_for_request(&agent_id, &conversation_id, None)
                .map_err(|e| e.to_string())
        })
    }
    .await
    .inspect_err(|_e| {
        let _ = registry.remove_chat_request(&request_id);
    })?;

    let recovery_note = {
        let conversation_id = conversation_id.clone();
        let agent_id = agent_id.clone();
        run_blocking(move || {
            conversation_store::build_recovery_developer_note(&agent_id, &conversation_id)
                .map_err(|e| e.to_string())
        })
        .await
        .ok()
        .flatten()
    };

    let mut tools_catalog =
        ToolCatalog::new_with_home_plugins(mcp_manager.inner().clone(), &config, &agent_config);

    // ── Resolve model early for dynamic LCM threshold computation ───────
    let resolved_model =
        resolve_prompt_counting_model(&config, &agent_config, req.model.as_deref());

    // ── Compute effective LCM thresholds (dynamic or manual) ────────────
    let effective_lcm_config = {
        let base = agent_config.context_management.to_lcm_config();
        if base.dynamic_thresholds {
            if let Some(ref model) = resolved_model {
                let budget = conversation_store::TokenBudget::for_model(
                    &model.provider.kind,
                    &model.model_id,
                );
                log::info!(
                    "LCM dynamic thresholds: model={} provider={} context_window={} (soft={}, hard={}, large_file={})",
                    model.model_id,
                    model.provider.kind,
                    budget.context_window,
                    budget.context_window / 2,
                    (budget.context_window as f64 * 0.85) as u32,
                    ((budget.context_window / 10) as u32).min(100_000),
                );
                base.with_dynamic_thresholds(budget.context_window)
            } else {
                log::warn!(
                    "LCM dynamic thresholds enabled but model resolution failed; falling back to manual thresholds"
                );
                base
            }
        } else {
            base
        }
    };

    // ── LCM: Initialize context management (always on) ──────────────────
    let lcm_engine = crate::lcm::open_lcm_engine_with_summarizer(
        &agent_id,
        &conversation_id,
        &effective_lcm_config,
        &config,
        &agent_config,
    )?;
    let agent_ctx = LcmAgentContext::new(lcm_engine);
    tools_catalog.set_context_tools(agent_ctx.engine().store().clone());
    let tools_catalog = Arc::new(tools_catalog);

    // Ensure LCM conversation metadata exists.
    let _ = agent_ctx
        .engine()
        .store()
        .ensure_conversation_meta(&conversation_id)
        .map_err(|e| {
            log::warn!(
                "Failed to ensure LCM conversation meta for '{}': {}",
                conversation_id,
                e
            )
        });

    log::info!(
        "LCM initialized for conversation '{}' (db: {:?})",
        conversation_id,
        agent_ctx.engine().store().db_path()
    );

    let user_message_ts = now_unix_ms();

    if jsonl_backup_enabled {
        let conversation_id = conversation_id.clone();
        let request_id = request_id.clone();
        let input_text = input_text.clone();
        let agent_id = agent_id.clone();
        run_blocking(move || {
            conversation_store::append_line(
                &agent_id,
                conversation_store::AppendLineInput {
                    conversation_id,
                    line: conversation_store::ConversationLine::User(
                        conversation_store::UserLine {
                            id: format!("msg-user-{request_id}"),
                            ts: user_message_ts,
                            request_id,
                            text: input_text,
                        },
                    ),
                },
            )
            .map_err(|e| e.to_string())
        })
        .await?;
    }

    // resolved_model was computed above (before LCM engine initialization).
    let model_id: Option<String> = resolved_model.as_ref().map(|m| m.model_id.clone());

    // ── Apply token budget truncation now that we know the model ──────
    if let Some(resolved_model) = resolved_model.as_ref() {
        let budget = conversation_store::TokenBudget::for_model(
            &resolved_model.provider.kind,
            &resolved_model.model_id,
        );
        context.input_items = conversation_store::truncate_items_to_budget(
            std::mem::take(&mut context.input_items),
            &budget,
        );
    }

    let context_token_count = if let Some(resolved_model) = resolved_model.as_ref() {
        let tool_context = ToolExecutionContext {
            agent_id: Some(agent_id.clone()),
            conversation_id: Some(conversation_id.clone()),
            ..Default::default()
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
        let mut system_items = resolved_model.prompt_assembly.system_items.clone();
        system_items.push(build_temporal_context_system_item(
            now_unix_ms(),
            user_message_ts,
        ));
        // Inject memory context if enabled.
        if agent_config.memory.enabled && agent_config.memory.auto_inject {
            let memory_base = crate::agentjax_home::agentjax_home_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from(".agentjax"))
                .join(&agent_config.memory.storage_dir);
            if let Ok(store) = crate::memory::MemoryStore::open(memory_base)
                && let Ok(Some(memory_item)) =
                    crate::memory::build_memory_context(&store, &agent_config.memory)
            {
                system_items.push(memory_item);
            }
        }
        let current_user_item = build_user_input_item(
            &resolved_model.provider.kind,
            &render_timed_message("Current user message", user_message_ts, &input_text),
        )?;

        match conversation_store::count_conversation_prompt_tokens(
            &resolved_model.model_id,
            Some(&resolved_model.system_prompt),
            &system_items,
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

    let stream_observer = ChatStreamObserver::new(
        agent_id.clone(),
        conversation_id.clone(),
        request_id.clone(),
        model_id,
        context_token_count,
        jsonl_backup_enabled,
    );
    let mut runtime_req = req.clone();
    runtime_req.client_metadata = sanitized_client_metadata;
    // Create sub-agent event channel: event_tx goes to tool execution,
    // event_rx is consumed by a forwarding task that emits to the frontend.
    let (sub_event_tx, mut sub_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::sub_agents::SubAgentEvent>();
    // Spawn forwarding task so sub-agent events reach the frontend.
    let forward_window = closure_window.clone();
    let forward_request_id = closure_request_id.clone();
    tokio::spawn(async move {
        let mut index = 0u64;
        while let Some(sub_event) = sub_event_rx.recv().await {
            let chat_event = crate::sub_agents::events::sub_agent_event_to_chat_stream_event(
                &sub_event,
                &forward_request_id,
                &mut index,
            );
            let _ = forward_window.emit("chat_stream_event", chat_event);
        }
    });

    // ── Street Event Channel ────────────────────────────────────────────
    let (street_event_tx, mut street_event_rx) =
        tokio::sync::mpsc::unbounded_channel::<crate::street::StreetEvent>();
    crate::street::StreetManager::register_event_channel(&conversation_id, street_event_tx);
    let street_fwd_window = closure_window.clone();
    let street_fwd_conv = conversation_id.clone();
    tokio::spawn(async move {
        let mut index = 0u64;
        while let Some(event) = street_event_rx.recv().await {
            let (kind, delta, tool_name) = match &event {
                crate::street::StreetEvent::Deposited {
                    item_id: _,
                    title,
                    priority,
                    ..
                } => (
                    "street_notification".to_string(),
                    Some(title.clone()),
                    Some(priority.as_str().to_string()),
                ),
                crate::street::StreetEvent::Cleared { count, .. } => {
                    ("street_cleared".to_string(), Some(count.to_string()), None)
                }
            };
            index += 1;
            let chat_event = crate::commands::chat::chat_events::ChatStreamEvent {
                request_id: street_fwd_conv.clone(),
                event_index: index,
                kind,
                delta,
                response_id: None,
                conversation_id: Some(street_fwd_conv.clone()),
                conversation_title: None,
                error: None,
                tool_call_id: None,
                tool_name,
                tool_display_name: None,
                tool_description: None,
                tool_icon: None,
                tool_arguments: None,
                tool_output: None,
                tool_status: None,
                tool_started_ts: None,
                tool_completed_ts: None,
                tool_duration_ms: None,
                context_token_count: None,
                phase: None,
                agent_id: None,
            };
            let _ = street_fwd_window.emit("chat_stream_event", chat_event);
        }
    });

    // ── Memory Agent Lifecycle ──────────────────────────────────────────
    // If memory is enabled and no memory agent exists for this conversation,
    // spawn a persistent background memory observer.
    if agent_config.memory.enabled {
        use crate::sub_agents::manager::SubAgentManager;
        use crate::sub_agents::types::SubAgentType;
        if SubAgentManager::get_memory_agent_for_conversation(&conversation_id).is_none() {
            let mem_agent_id = format!("mem_{}", uuid::Uuid::new_v4().simple());
            let (mem_signal_tx, mem_signal_rx) = tokio::sync::watch::channel(None);
            let mem_spec = crate::sub_agents::types::SubAgentSpec {
                agent_id: mem_agent_id.clone(),
                parent_conversation_id: conversation_id.clone(),
                subagent_type: SubAgentType::Memory,
                prompt: "Background memory observer".to_string(),
                delegated_scope: vec!["lcm".to_string(), "memory".to_string()],
                kept_work: vec!["memory_update".to_string()],
                max_turns: 3,
                max_retries: 0,
                use_worktree: false,
                model_id: None,
                parent_request_id: request_id.clone(),
                persistent: true,
            };
            let mem_task = SubAgentManager::register(mem_spec.clone());
            // Store the signal sender in the task for signal dispatch.
            if let Ok(mut tx_guard) = mem_task.memory_signal_tx.lock() {
                *tx_guard = Some(mem_signal_tx);
            }
            let mem_config = Arc::new(config.clone());
            let mem_agent_config = Arc::new(agent_config.clone());
            tokio::spawn(async move {
                crate::sub_agents::runner::run_memory_agent(
                    mem_spec,
                    mem_config,
                    mem_agent_config,
                    mem_signal_rx,
                )
                .await;
            });
            log::info!("Memory agent spawned for conv={}", conversation_id);
        }
    }

    let mut is_first_turn = true;
    let mut current_input_items = context.input_items.clone();
    let mut last_response: Option<crate::provider_api::types::ResponseStreamResult> = None;
    let mut last_final_token_count = 0usize;

    'resume_loop: loop {
        if *cancel_rx.borrow() {
            break 'resume_loop;
        }

        if !is_first_turn {
            if let Err(e) = agent_ctx.rebuild(&conversation_id).await {
                log::warn!("Failed to rebuild LCM context for auto-resume: {e}");
            }
            current_input_items = agent_ctx.context_items();
        }

        // ── Collect Street notifications ────────────────────────────────────
        let street_dev_items: Vec<Value> = if agent_config.context_management.street_enabled {
            let pending = crate::street::StreetManager::collect_pending(&conversation_id);
            if !pending.is_empty() {
                let count = pending.len();
                let formatted = crate::street::format_street_items(&pending);
                crate::street::StreetManager::mark_delivered(&conversation_id);
                vec![crate::street::build_street_context_system_item(
                    count, &formatted,
                )]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let loop_window = window.clone();
        let loop_request_id = request_id.clone();
        let loop_conversation_id = conversation_id.clone();
        let loop_stream_observer_for_callback = stream_observer.clone();
        let event_index_clone = Arc::clone(&event_index);

        let run_result = crate::runtime::AgentRuntime::run_turn(
            &config,
            &agent_config,
            &runtime_req,
            &conversation_id,
            user_message_ts,
            current_input_items.clone(),
            recovery_note.clone(),
            &tools_catalog,
            &agent_ctx,
            &mut cancel_rx,
            Some(sub_event_tx.clone()),
            street_dev_items,
            !is_first_turn, // is_auto_resume
            move |event| {
                let event_token_count = loop_stream_observer_for_callback.handle_provider_event(&event);
                let mut idx = event_index_clone.load(std::sync::atomic::Ordering::SeqCst);
                let res = emit_mapped_stream_event(
                    &loop_window,
                    &loop_request_id,
                    &loop_conversation_id,
                    &mut idx,
                    event,
                    event_token_count,
                );
                event_index_clone.store(idx, std::sync::atomic::Ordering::SeqCst);
                res.map_err(Into::into)
            },
        )
        .await;

        if *cancel_rx.borrow() {
            break 'resume_loop;
        }

        let (response, _timeline_events) = run_result?;

        // ── Spawn registered sub-agents ───────────────────────────────────────
        // Sub-agent tasks registered during tool execution are still Pending.
        // We now spawn their runners so they execute concurrently after the
        // main turn completes.
        {
            use crate::sub_agents::runner::run_sub_agent;
            let pending =
                crate::sub_agents::manager::SubAgentManager::collect_pending(&conversation_id);
            if !pending.is_empty() {
                let tools_catalog_arc = Arc::clone(&tools_catalog);
                let sub_semaphore = crate::sub_agents::manager::sub_agent_semaphore();
                for (task, spec) in pending {
                    let agent_id = spec.agent_id.clone();
                    let spawn_config = Arc::new(config.clone());
                    let spawn_agent_config = Arc::new(agent_config.clone());
                    let spawn_catalog = Arc::clone(&tools_catalog_arc);
                    let spawn_event_tx = sub_event_tx.clone();
                    let sem_perm = sub_semaphore;
                    let _handle = tokio::spawn(async move {
                        // Acquire concurrency permit before starting execution.
                        let _permit = sem_perm.acquire().await;
                        run_sub_agent(
                            task,
                            spec,
                            spawn_config,
                            spawn_agent_config,
                            spawn_catalog,
                            spawn_event_tx,
                        )
                        .await;
                    });
                    log::info!(
                        "Sub-agent {} spawned for conv={}",
                        agent_id,
                        conversation_id
                    );
                }
            }
        }

        let final_token_count = stream_observer.persist_final_token_usage(&response);
        last_response = Some(response.clone());
        last_final_token_count = final_token_count;

        // ── Signal the memory agent ───────────────────────────────────────────
        // After the main turn completes, notify the memory agent so it can
        // evaluate the conversation and write/update memories.
        if agent_config.memory.enabled {
            crate::sub_agents::manager::SubAgentManager::signal_memory_agent(
                &conversation_id,
                crate::sub_agents::types::MemoryAgentSignal::TurnCompleted,
            );
        }

        log::info!(
            "chat_stream turn complete: conv={} req={} text_len={} resp_id={} output_items={}",
            conversation_id,
            request_id,
            response.output_text.len(),
            response.response_id,
            response.output_items.len(),
        );

        // ── Check Auto-Resume Status ──────────────────────────────────────────
        let still_has_active = crate::sub_agents::manager::SubAgentManager::list(Some(&conversation_id))
            .iter()
            .any(|s| {
                s.subagent_type != crate::sub_agents::types::SubAgentType::Memory.as_str()
                    && (s.status == crate::sub_agents::types::SubAgentStatus::Running.as_str()
                        || s.status == crate::sub_agents::types::SubAgentStatus::Pending.as_str())
            });

        if !still_has_active {
            break 'resume_loop;
        }

        // Emit waiting_for_subagents transitional event since we have active subagents.
        let mut idx = event_index.load(std::sync::atomic::Ordering::SeqCst);
        window
            .emit(
                "chat_stream_event",
                ChatStreamEvent {
                    request_id: request_id.clone(),
                    event_index: next_event_index(&mut idx),
                    kind: "waiting_for_subagents".to_string(),
                    delta: Some(response.output_text.clone()),
                    response_id: Some(response.response_id.clone()),
                    conversation_id: Some(conversation_id.clone()),
                    conversation_title: None,
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
                    agent_id: None,
                },
            )
            .map_err(|e| format!("Failed to emit stream waiting event: {e}"))?;
        event_index.store(idx, std::sync::atomic::Ordering::SeqCst);

        let notifier = crate::street::StreetManager::get_or_create_notifier(&conversation_id);
        loop {
            if *cancel_rx.borrow() {
                break 'resume_loop;
            }

            if crate::street::StreetManager::is_auto_resume(&conversation_id) {
                is_first_turn = false;
                continue 'resume_loop;
            }

            let still_active = crate::sub_agents::manager::SubAgentManager::list(Some(&conversation_id))
                .iter()
                .any(|s| {
                    s.subagent_type != crate::sub_agents::types::SubAgentType::Memory.as_str()
                        && (s.status == crate::sub_agents::types::SubAgentStatus::Running.as_str()
                            || s.status == crate::sub_agents::types::SubAgentStatus::Pending.as_str())
                });
            if !still_active {
                break 'resume_loop;
            }

            tokio::select! {
                _ = notifier.notified() => {
                    // Loop again to check is_auto_resume
                }
                _ = cancel_rx.changed() => {
                    break 'resume_loop;
                }
            }
        }
    }

    // ── Generate conversation title if needed ────────────────────────────
    let conversation_title: Option<String> = None;
    if !registry.is_conversation_deleted(&conversation_id)? {
        schedule_title_generation(
            window.clone(),
            window.app_handle().clone(),
            full_config.clone(),
            agent_id.clone(),
            conversation_id.clone(),
            request_id.clone(),
        );
    }

    // ── Emit final done event ────────────────────────────────────────────
    let final_response = last_response.ok_or_else(|| "No turns completed".to_string())?;

    let mut idx = event_index.load(std::sync::atomic::Ordering::SeqCst);
    window
        .emit(
            "chat_stream_event",
            ChatStreamEvent {
                request_id: request_id.clone(),
                event_index: next_event_index(&mut idx),
                kind: "done".to_string(),
                delta: Some(final_response.output_text.clone()),
                response_id: Some(final_response.response_id.clone()),
                conversation_id: Some(conversation_id.clone()),
                conversation_title,
                context_token_count: Some(last_final_token_count),
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
                agent_id: None,
            },
        )
        .map_err(|e| format!("Failed to emit final stream done event: {e}"))?;
    event_index.store(idx, std::sync::atomic::Ordering::SeqCst);

    let _ = registry.remove_chat_request(&request_id)?;

    Ok(ChatResponse {
        response_id: final_response.response_id,
        output_text: final_response.output_text,
        conversation_id,
        conversation_title: None,
        context_token_count: last_final_token_count,
    })
}

/// Resolve the effective agent_id from a request, falling back to the default.
fn resolve_agent_id(agent_id: Option<&str>) -> String {
    agent_id
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::config::constants::DEFAULT_AGENT_ID)
        .to_string()
}

#[tauri::command]
pub fn list_conversations(
    agent_id: Option<String>,
) -> Result<Vec<conversation_store::ConversationSummary>, String> {
    let agent_id = resolve_agent_id(agent_id.as_deref());
    conversation_store::list_conversations(&agent_id).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn load_conversation(
    mcp_manager: State<'_, std::sync::Arc<crate::mcp::McpManager>>,
    req: LoadConversationRequest,
) -> Result<Option<conversation_store::ConversationDetail>, String> {
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    let conversation_id = req.conversation_id.clone();
    let mut detail = conversation_store::load_conversation(&agent_id, &req.conversation_id)?;
    if let Some(detail_ref) = detail.as_mut() {
        detail_ref.context_token_count =
            match conversation_store::load_conversation_token_usage_count(
                &agent_id,
                &conversation_id,
            )? {
                Some(count) => count,
                None => {
                    load_conversation_prompt_token_count(
                        &agent_id,
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
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    conversation_store::load_conversation_dynamic_tools(&agent_id, &req.conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn replace_conversation_dynamic_tools(
    req: ReplaceConversationDynamicToolsRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    validate_conversation_dynamic_tools(&req.tools)?;
    conversation_store::ensure_conversation(&agent_id, &req.conversation_id)?;
    conversation_store::update_conversation_dynamic_tools(
        &agent_id,
        &req.conversation_id,
        req.tools,
    )?;
    conversation_store::load_conversation_dynamic_tools(&agent_id, &req.conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn upsert_conversation_dynamic_tool(
    req: UpsertConversationDynamicToolRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    validate_conversation_dynamic_tools(std::slice::from_ref(&req.tool))?;
    conversation_store::ensure_conversation(&agent_id, &req.conversation_id)?;
    conversation_store::upsert_conversation_dynamic_tool(
        &agent_id,
        &req.conversation_id,
        req.tool,
    )?;
    conversation_store::load_conversation_dynamic_tools(&agent_id, &req.conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn remove_conversation_dynamic_tool(
    req: RemoveConversationDynamicToolRequest,
) -> Result<Vec<conversation_store::ConversationDynamicTool>, String> {
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    let tool_name = req.tool_name.trim();
    if tool_name.is_empty() {
        return Err("toolName cannot be empty".to_string());
    }
    conversation_store::ensure_conversation(&agent_id, &req.conversation_id)?;
    conversation_store::remove_conversation_dynamic_tool(
        &agent_id,
        &req.conversation_id,
        tool_name,
    )?;
    conversation_store::load_conversation_dynamic_tools(&agent_id, &req.conversation_id)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn rename_conversation(
    registry: State<'_, ChatRequestRegistry>,
    req: RenameConversationRequest,
) -> Result<conversation_store::ConversationSummary, String> {
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    let _ = registry.cancel_title_request(&req.conversation_id)?;

    conversation_store::rename_conversation(&agent_id, &req.conversation_id, &req.title)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn delete_conversation(
    registry: State<'_, ChatRequestRegistry>,
    req: DeleteConversationRequest,
) -> Result<bool, String> {
    let agent_id = resolve_agent_id(req.agent_id.as_deref());
    registry.mark_conversation_deleted(&req.conversation_id)?;
    registry.cancel_conversation_tasks(&req.conversation_id)?;

    conversation_store::delete_conversation(&agent_id, &req.conversation_id)
        .map_err(|e| e.to_string())
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
        let _guard = config::test_env_lock().blocking_lock();
        let _home = setup_test_home();
        let conversation_id = format!("conv-dtool-cmd-{}", Uuid::new_v4());

        let replaced = replace_conversation_dynamic_tools(ReplaceConversationDynamicToolsRequest {
            agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
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
            agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
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
            agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
            conversation_id: conversation_id.clone(),
        })
        .expect("load dynamic tools");
        assert_eq!(loaded.len(), 2);

        let after_remove = remove_conversation_dynamic_tool(RemoveConversationDynamicToolRequest {
            agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
            conversation_id: conversation_id.clone(),
            tool_name: "math_alias".to_string(),
        })
        .expect("remove dynamic tool");
        assert_eq!(after_remove.len(), 1);
        assert_eq!(after_remove[0].name, "time_alias");
    }
}
