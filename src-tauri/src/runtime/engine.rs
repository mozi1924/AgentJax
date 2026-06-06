mod output;
mod presentations;
mod request;
mod tool_state;
mod turn;

use super::AgentRuntime;
use super::agent_context::AgentContext;
use super::stream_collection::collect_provider_turn;
use super::tool_archiving::archive_unavailable_historical_tool_calls;
use super::tool_execution::ToolExecutionScheduler;
use super::tool_parsing::describe_item_shape;
use crate::commands::chat::ChatRequest;
use crate::config::{AgentConfig, AppConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::message_phase::AssistantPhase;
use crate::provider_api::types::{ProviderStreamEvent, ResponseStreamResult};
use crate::time_context::{build_temporal_context_system_item, render_timed_message};
use crate::tools::{ToolCatalog, ToolExecutionContext};
use output::{
    extract_assistant_messages_from_items, resolve_hop_phase, select_final_output_text,
    strip_commentary_prefixes,
};
use presentations::enrich_tool_stream_event;
use request::{build_request, ensure_tool_call_output_pairs};
use serde_json::Value;
use std::sync::Arc;
use tokio::sync::watch;
use tool_state::apply_tool_state_changes;
use turn::TurnAccumulator;

impl AgentRuntime {
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn<F>(
        config: &AppConfig,
        agent: &AgentConfig,
        req: &ChatRequest,
        conversation_id: &str,
        user_message_ts: i64,
        context_items: Vec<Value>,
        recovery_note: Option<Value>,
        tools_catalog: &ToolCatalog,
        context: &dyn AgentContext,
        cancel_rx: &mut watch::Receiver<bool>,
        sub_agent_event_tx: Option<
            tokio::sync::mpsc::UnboundedSender<crate::sub_agents::SubAgentEvent>,
        >,
        street_items: Vec<Value>,
        is_auto_resume: bool,
        mut on_event: F,
    ) -> AgentJaxResult<(ResponseStreamResult, Vec<Value>)>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), AgentJaxError> + Send + 'static,
    {
        let resolved_model =
            config.resolve_model_profile_with_agent(req.model.as_deref(), agent)?;
        let provider_capabilities =
            crate::provider_api::get_capabilities(&resolved_model.provider.kind)?;
        // When a model explicitly overrides the API protocol to chat_completions,
        // tool schemas must follow the Chat Completions format (wrapped in "function" key).
        // Otherwise, use the provider's default tool schema format.
        let tool_schema_format = match resolved_model.api_protocol.as_deref() {
            Some(p) if p.trim().eq_ignore_ascii_case("chat_completions") => {
                crate::tools::ToolSchemaFormat::ChatCompletions
            }
            _ => crate::provider_api::get_tool_schema_format(&resolved_model.provider.kind)?,
        };
        let provider_kind = &resolved_model.provider.kind;
        let mut system_items = resolved_model.prompt_assembly.system_items.clone();
        let request_started_at_unix_ms = crate::conversation_store_utils::now_unix_ms();
        system_items.push(build_temporal_context_system_item(
            request_started_at_unix_ms,
            user_message_ts,
        ));

        // Detect sub-agent context from conversation ID pattern.
        // Sub-agent conversations use the format:
        //   "{parent}/sub-agent/{type}/{agent_id}"
        // where {type} is the SubAgentType (e.g., "explore", "memory", "implement").
        // When detected, `sub_agent_id` is set in ToolExecutionContext which gates
        // `lcm_expand` access. Note: LCM tools (grep/describe/expand) always read
        // from the **parent conversation's** store — sub-agents do not need their
        // own isolated store for tool access since they are short-lived.
        // The sub-agent type is also propagated so context tools like memory_write
        // can be gated appropriately.
        let is_sub_agent = conversation_id.contains("/sub-agent/");
        let sub_agent_type: Option<String> = if is_sub_agent {
            // Extract type from ".../sub-agent/{type}/{id}"
            conversation_id
                .split("/sub-agent/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .map(|s| s.to_string())
        } else {
            None
        };
        let is_memory_sub_agent = sub_agent_type.as_deref() == Some("memory");
        let tool_context = ToolExecutionContext {
            conversation_id: Some(conversation_id.to_string()),
            model_id: Some(resolved_model.model_id.clone()),
            app_config: Some(Arc::new(config.clone())),
            sub_agent_id: if is_sub_agent {
                // Extract agent_id (last segment) from the conversation_id path.
                conversation_id.rsplit('/').next().map(|s| s.to_string())
            } else {
                None
            },
            sub_agent_type: sub_agent_type.clone(),
            is_memory_sub_agent,
            sub_agent_event_tx: sub_agent_event_tx.clone(),
            ..Default::default()
        };
        let mut mounted_mcp_servers = tools_catalog.load_persisted_mounted_servers(&tool_context);
        let initial_snapshot = tools_catalog
            .snapshot_with_format_and_mounted_servers(
                tool_schema_format,
                &tool_context,
                &mounted_mcp_servers,
            )
            .await;
        let _ = archive_unavailable_historical_tool_calls(
            context_items.clone(),
            initial_snapshot.active_tool_names(),
        );

        let mut accumulator = TurnAccumulator::new();
        let mut accumulated_context: Vec<Value> = Vec::new();
        let mut repeated_failed_tool_signatures = std::collections::HashMap::new();
        let mut final_output_text = String::new();
        let mut commentary_history: Vec<String> = Vec::new();
        // MCP server mounts are conversation-scoped and persisted in
        // metadata.json so later turns and app restarts can recover the same
        // mounted tool surface until the agent explicitly unmounts it.
        let mut turn_idx = 0usize;
        let max_turns = agent.max_tool_turns;

        // ── Seed active context with existing conversation history ──
        context.rebuild(conversation_id).await.ok();

        // ── Persist the current user message ───────────────────────────
        if !is_auto_resume {
            let user_text = req.input.trim();
            let request_id = req.request_id.as_deref().unwrap_or("unknown");
            let mut user_msg = crate::lcm::types::StoredMessage::new(
                crate::lcm::types::MessageId::new(),
                conversation_id,
                crate::lcm::types::MessageRole::User,
                user_text,
                crate::lcm::types::estimate_tokens(user_text),
                user_message_ts,
            );
            user_msg.metadata.insert(
                "request_id".to_string(),
                serde_json::Value::String(request_id.to_string()),
            );
            if let Err(e) = context.persist_message(&user_msg).await {
                log::warn!("Failed to persist user message for turn: {e}");
            }
        }

        // ── Build prefix items sent once per turn (not per hop) ──────────
        // System items (prompt blocks, temporal context) and recovery note
        // are included in hop 1. Subsequent hops use only LCM context since
        // the model already has these instructions from the first hop.
        let hop_prefix: Vec<Value> = {
            let mut prefix = std::mem::take(&mut system_items);
            if let Some(note) = recovery_note {
                prefix.push(note);
            }
            // Inject Street notifications as user-role items.
            // We use user role (not system) to avoid prompt injection risks
            // from dynamic async result content — the model treats these as
            // data/observations rather than authoritative instructions.
            prefix.extend(street_items);
            prefix
        };

        'turn_loop: loop {
            if max_turns > 0 && turn_idx >= max_turns {
                return Err(crate::error::AgentJaxError::internal(
                    "Maximum turn execution limit reached",
                ));
            }
            turn_idx += 1;

            // ── Determine input for this hop ──────────────────────────────
            // All hops use the active context as the single source of truth
            // for conversation history. Hop 1 additionally includes the prefix
            // (system items + recovery note) and a formatted user message.
            let active_context = context.context_items();
            let lcm_context = if active_context.is_empty() {
                accumulated_context.clone()
            } else {
                active_context
            };

            let input_items = if turn_idx == 1 {
                // Hop 1: prefix + LCM history + rendered user input (with timestamp).
                let mut items = hop_prefix.clone();
                if is_auto_resume {
                    items.extend(lcm_context);
                    items
                } else {
                    // Keep all historical user messages — the model needs to
                    // see what the user previously asked. Only the very last
                    // user item is skipped because it will be re-rendered
                    // below with a "Current user message" label.
                    let mut seen_current_user = false;
                    let history_items: Vec<Value> = lcm_context
                        .into_iter()
                        .rev()
                        .filter(|item| {
                            if matches!(item.get("role").and_then(|v| v.as_str()), Some("user")) {
                                if !seen_current_user {
                                    seen_current_user = true;
                                    return false; // skip the most recent user message
                                }
                            }
                            true
                        })
                        .collect::<Vec<_>>()
                        .into_iter()
                        .rev()
                        .collect();
                    items.extend(history_items);
                    items.push(crate::provider_api::build_user_input_item(
                        provider_kind,
                        &render_timed_message(
                            "Current user message",
                            user_message_ts,
                            req.input.trim(),
                        ),
                    )?);
                    items
                }
            } else {
                lcm_context
            };

            // Freeze tool visibility per hop. A tool execution may mount an MCP
            // server, and those newly available tools should appear only on the
            // next hop, never halfway through the current provider response.
            let tool_snapshot = tools_catalog
                .snapshot_with_format_and_mounted_servers(
                    tool_schema_format,
                    &tool_context,
                    &mounted_mcp_servers,
                )
                .await;

            // ── Archive tool calls for unavailable tools ──────────────────
            // When an MCP server is unmounted or a plugin is disabled between
            // turns, historical function_call / function_call_output items
            // for those tools become orphaned. Replace them with system
            // notes so the model knows the tool is gone and doesn't try to
            // re-call it, while still preserving the historical context.
            // This also prevents tool-result injection via stale call_id
            // matching for tools that no longer exist.
            let input_items = archive_unavailable_historical_tool_calls(
                input_items,
                tool_snapshot.active_tool_names(),
            );

            let stream_request =
                build_request(req, input_items.clone(), tool_snapshot.schemas().to_vec());
            let mut tool_scheduler = ToolExecutionScheduler::new(
                conversation_id,
                tool_snapshot.clone(),
                provider_capabilities.supports_parallel_tool_calls,
                cancel_rx,
            );
            let collected = collect_provider_turn(
                config,
                agent,
                provider_kind,
                &resolved_model.provider_key,
                &stream_request,
                cancel_rx,
                &mut |event| on_event(enrich_tool_stream_event(event, &tool_snapshot)),
                Some(&mut tool_scheduler),
                &repeated_failed_tool_signatures,
            )
            .await?;

            accumulator.record_hop(&collected.response_result);
            if let (Some(hop_usage), Some(aggregate_usage)) = (
                collected.response_result.usage.clone(),
                accumulator.usage.clone(),
            ) {
                on_event(ProviderStreamEvent::UsageUpdated {
                    response_id: collected.response_result.response_id.clone(),
                    usage: hop_usage,
                    aggregate_usage,
                })?;
            }

            let is_final_hop = collected.pending_tools.is_empty();
            let hop_messages =
                extract_assistant_messages_from_items(&collected.response_result.output_items);
            // Collected (text, resolved_phase) pairs for LCM persistence.
            // Uses resolved phases (not raw output_item phases) so stored data
            // matches what the frontend receives via HopAssistantText events.
            let mut hop_messages_for_lcm: Vec<(String, Option<AssistantPhase>)> = Vec::new();
            if hop_messages.is_empty() && !collected.response_result.output_text.is_empty() {
                let phase = resolve_hop_phase(None, is_final_hop);
                let emitted_text = if phase == Some(AssistantPhase::Commentary) {
                    collected.response_result.output_text.clone()
                } else {
                    strip_commentary_prefixes(
                        &collected.response_result.output_text,
                        &commentary_history,
                    )
                };
                on_event(ProviderStreamEvent::HopAssistantText {
                    text: emitted_text.clone(),
                    phase,
                    response_id: collected.response_result.response_id.clone(),
                })?;
                hop_messages_for_lcm.push((emitted_text.clone(), phase));
                if phase != Some(AssistantPhase::Commentary) {
                    final_output_text = emitted_text;
                } else {
                    commentary_history.push(emitted_text);
                }
            } else {
                for (text, phase) in hop_messages {
                    let resolved_phase = resolve_hop_phase(phase, is_final_hop);
                    let emitted_text = if resolved_phase == Some(AssistantPhase::Commentary) {
                        text.clone()
                    } else {
                        strip_commentary_prefixes(&text, &commentary_history)
                    };
                    on_event(ProviderStreamEvent::HopAssistantText {
                        text: emitted_text.clone(),
                        phase: resolved_phase,
                        response_id: collected.response_result.response_id.clone(),
                    })?;
                    hop_messages_for_lcm.push((emitted_text.clone(), resolved_phase));
                    if resolved_phase != Some(AssistantPhase::Commentary) {
                        final_output_text = emitted_text;
                    } else {
                        commentary_history.push(text);
                    }
                }
            }

            // ── Persist this hop's messages BEFORE the is_final_hop break ──
            // Must run for EVERY hop — including the final hop without tool calls.
            // Otherwise the final answer is never stored.
            {
                let engine = context;
                let now_ms = crate::conversation_store_utils::now_unix_ms();
                let lcm_conv_id = conversation_id.to_string();
                let request_id = req.request_id.as_deref().unwrap_or("unknown");
                let response_id = &collected.response_result.response_id;
                let mut batch_messages: Vec<crate::lcm::types::StoredMessage> = Vec::new();

                // Collect assistant text from structured output_items.
                for (text, phase) in &hop_messages_for_lcm {
                    if !text.trim().is_empty() {
                        let mut msg = crate::lcm::types::StoredMessage::new(
                            crate::lcm::types::MessageId::new(),
                            &lcm_conv_id,
                            crate::lcm::types::MessageRole::Assistant,
                            text,
                            crate::lcm::types::estimate_tokens(text),
                            now_ms,
                        );
                        msg.metadata.insert(
                            "request_id".to_string(),
                            serde_json::Value::String(request_id.to_string()),
                        );
                        msg.metadata.insert(
                            "response_id".to_string(),
                            serde_json::Value::String(response_id.clone()),
                        );
                        if let Some(p) = phase {
                            msg.metadata.insert(
                                "phase".to_string(),
                                serde_json::Value::String(p.as_str().to_string()),
                            );
                        }
                        batch_messages.push(msg);
                    }
                }

                // ── Extract reasoning/thinking from output_items ─────────
                // Store thinking content directly on the StoredMessage so it
                // survives restarts without needing a separate reasoning_chains
                // table lookup.
                let hop_thinking_text: Option<String> = {
                    let parts: Vec<&str> = collected
                        .response_result
                        .output_items
                        .iter()
                        .filter(|item| {
                            item.get("type").and_then(Value::as_str) == Some("reasoning")
                        })
                        .filter_map(|item| item.get("text").and_then(Value::as_str))
                        .filter(|text| !text.trim().is_empty())
                        .collect();
                    if parts.is_empty() {
                        None
                    } else {
                        Some(parts.join("\n"))
                    }
                };

                // Attach thinking to all assistant messages in this batch.
                if let Some(ref thinking_text) = hop_thinking_text {
                    for msg in &mut batch_messages {
                        if msg.role == crate::lcm::types::MessageRole::Assistant {
                            msg.thinking = Some(thinking_text.clone());
                        }
                    }
                }

                // ── Lossless invariant guard ─────────────────────────────
                let fallback_text: Option<String> = if hop_messages_for_lcm.is_empty()
                    && !collected.response_result.output_text.trim().is_empty()
                {
                    Some(collected.response_result.output_text.trim().to_string())
                } else if is_final_hop && !final_output_text.trim().is_empty() {
                    let already_captured = hop_messages_for_lcm
                        .iter()
                        .any(|(t, _)| t.trim() == final_output_text.trim());
                    if !already_captured {
                        Some(final_output_text.trim().to_string())
                    } else {
                        None
                    }
                } else {
                    None
                };

                if let Some(text) = fallback_text {
                    let phase = resolve_hop_phase(None, is_final_hop);
                    let phase_str = phase.map(|p| p.as_str().to_string()).unwrap_or_else(|| {
                        if is_final_hop {
                            "final_answer".to_string()
                        } else {
                            "commentary".to_string()
                        }
                    });
                    let mut msg = crate::lcm::types::StoredMessage::new(
                        crate::lcm::types::MessageId::new(),
                        &lcm_conv_id,
                        crate::lcm::types::MessageRole::Assistant,
                        &text,
                        crate::lcm::types::estimate_tokens(&text),
                        now_ms,
                    );
                    msg.metadata.insert(
                        "request_id".to_string(),
                        serde_json::Value::String(request_id.to_string()),
                    );
                    msg.metadata.insert(
                        "response_id".to_string(),
                        serde_json::Value::String(response_id.clone()),
                    );
                    msg.metadata
                        .insert("phase".to_string(), serde_json::Value::String(phase_str));
                    // Attach thinking directly to the fallback message.
                    if let Some(ref thinking_text) = hop_thinking_text {
                        msg.thinking = Some(thinking_text.clone());
                    }
                    batch_messages.push(msg);
                }

                if !batch_messages.is_empty()
                    && let Err(e) = engine.persist_messages(&batch_messages).await
                {
                    log::warn!("Failed to persist {} messages: {}", batch_messages.len(), e);
                }
            }

            // ── No tools → final response reached ─────────────────────────
            if is_final_hop {
                if final_output_text.is_empty() {
                    final_output_text = select_final_output_text(
                        &extract_assistant_messages_from_items(
                            &collected.response_result.output_items,
                        ),
                        &collected.response_result.output_text,
                        &commentary_history,
                    );
                }
                break;
            }

            // ── Await tools that were scheduled as soon as their arguments completed ──
            tool_scheduler
                .schedule_pending_tools(collected.pending_tools, &repeated_failed_tool_signatures);
            let executed_batch = tool_scheduler
                .finish(
                    provider_kind,
                    &mut repeated_failed_tool_signatures,
                    &mut on_event,
                )
                .await?;

            // ── Persist tool-call hop messages ───────────────────────────
            {
                let engine = context;
                let now_ms = crate::conversation_store_utils::now_unix_ms();
                let lcm_conv_id = conversation_id.to_string();
                let tool_request_id = req.request_id.as_deref().unwrap_or("unknown");
                let lcm_tool_results = executed_batch.tool_results_items.clone();
                let lcm_tool_calls = executed_batch.executed_tool_call_items.clone();
                let mut batch_messages: Vec<crate::lcm::types::StoredMessage> = Vec::new();

                let tool_name_by_call_id: std::collections::HashMap<String, String> =
                    lcm_tool_calls
                        .iter()
                        .filter_map(|item| {
                            let call_id = item
                                .get("call_id")
                                .and_then(|v| v.as_str())
                                .map(String::from)?;
                            let name = item
                                .get("name")
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            Some((call_id, name))
                        })
                        .collect();

                for item in &lcm_tool_calls {
                    let call_id = item
                        .get("call_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown");
                    let name = item.get("name").and_then(|v| v.as_str()).unwrap_or("");
                    let arguments = item
                        .get("arguments")
                        .and_then(|v| v.as_str())
                        .unwrap_or("{}");
                    let mut metadata = std::collections::BTreeMap::new();
                    metadata.insert(
                        "message_type".to_string(),
                        serde_json::Value::String("function_call".to_string()),
                    );
                    metadata.insert(
                        "call_id".to_string(),
                        serde_json::Value::String(call_id.to_string()),
                    );
                    metadata.insert(
                        "tool_name".to_string(),
                        serde_json::Value::String(name.to_string()),
                    );
                    metadata.insert(
                        "arguments".to_string(),
                        serde_json::Value::String(arguments.to_string()),
                    );
                    metadata.insert(
                        "request_id".to_string(),
                        serde_json::Value::String(tool_request_id.to_string()),
                    );
                    let mut msg = crate::lcm::types::StoredMessage::new(
                        crate::lcm::types::MessageId::new(),
                        &lcm_conv_id,
                        crate::lcm::types::MessageRole::Tool,
                        arguments,
                        crate::lcm::types::estimate_tokens(arguments),
                        now_ms,
                    );
                    msg.metadata = metadata;
                    batch_messages.push(msg);
                }

                for item in &lcm_tool_results {
                    if let Some(output_str) = item.get("output").and_then(|v| v.as_str()) {
                        let call_id = item
                            .get("call_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("unknown");
                        let tool_name = tool_name_by_call_id
                            .get(call_id)
                            .map(String::as_str)
                            .unwrap_or("unknown");
                        let mut metadata = std::collections::BTreeMap::new();
                        metadata.insert(
                            "message_type".to_string(),
                            serde_json::Value::String("function_call_output".to_string()),
                        );
                        metadata.insert(
                            "call_id".to_string(),
                            serde_json::Value::String(call_id.to_string()),
                        );
                        metadata.insert(
                            "tool_name".to_string(),
                            serde_json::Value::String(tool_name.to_string()),
                        );
                        metadata.insert(
                            "request_id".to_string(),
                            serde_json::Value::String(tool_request_id.to_string()),
                        );
                        let mut msg = crate::lcm::types::StoredMessage::new(
                            crate::lcm::types::MessageId::new(),
                            &lcm_conv_id,
                            crate::lcm::types::MessageRole::Tool,
                            output_str,
                            crate::lcm::types::estimate_tokens(output_str),
                            now_ms,
                        );
                        msg.metadata = metadata;
                        batch_messages.push(msg);
                    }
                }

                if !batch_messages.is_empty()
                    && let Err(e) = engine.persist_messages(&batch_messages).await
                {
                    log::warn!(
                        "Failed to persist tool batch of {} messages: {}",
                        batch_messages.len(),
                        e
                    );
                }
            }

            accumulator
                .timeline_events
                .extend(executed_batch.timeline_events);
            apply_tool_state_changes(&mut mounted_mcp_servers, executed_batch.state_changes);
            if let Err(err) = tools_catalog.persist_mounted_servers(
                crate::config::constants::DEFAULT_AGENT_ID,
                conversation_id,
                &mounted_mcp_servers,
            ) {
                log::warn!(
                    "Failed to persist mounted MCP servers for conversation '{}': {}",
                    conversation_id,
                    err
                );
            }

            // ── Build this hop's delta items ──────────────────────────────
            let hop_delta = crate::provider_api::compose_tool_continuation_input(
                provider_kind,
                &collected.response_result.output_items,
                executed_batch.tool_results_items,
            )?;
            let hop_delta =
                ensure_tool_call_output_pairs(hop_delta, &executed_batch.executed_tool_call_items);

            let delta_shape: Vec<String> = hop_delta.iter().map(describe_item_shape).collect();
            log::debug!(
                "Tool hop {} for provider '{}': delta={} total_context_items={}",
                turn_idx,
                resolved_model.provider_key,
                delta_shape.join(", "),
                accumulated_context.len() + hop_delta.len(),
            );

            // ── Accumulate for the next continuation ──────────────────────
            // Critical fix: the next hop MUST include the FULL accumulated
            // context (hop 1 input + all prior deltas), not just this hop's delta.
            if turn_idx == 1 {
                // First accumulation: seed with hop 1 input items.
                accumulated_context = input_items.clone();
            }
            accumulated_context.extend(hop_delta.clone());

            // ── Collect output items for the final result ─────────────────
            // Include the full hop delta (reasoning, function_call,
            // function_call_output) so the frontend can reconstruct the
            // complete tool-call timeline.
            accumulator.absorb_continuation_batch(&hop_delta);

            if *cancel_rx.borrow() {
                break 'turn_loop;
            }
        }

        // Per-hop text was already emitted via HopAssistantText events.
        // The final result carries only output_items for the caller.
        let final_res = ResponseStreamResult {
            response_id: accumulator.last_response_id,
            output_text: final_output_text,
            output_items: accumulator.output_items,
            usage: accumulator.usage,
            usage_hops: accumulator.usage_hops,
            provider_key: resolved_model.provider_key.clone(),
            model_profile: resolved_model.profile_key.clone(),
            model_id: resolved_model.model_id.clone(),
            capabilities: provider_capabilities,
        };

        Ok((final_res, accumulator.timeline_events))
    }
}

#[cfg(test)]
mod tests {
    use super::output::{select_final_output_text, strip_commentary_prefixes};
    use super::presentations::merge_tool_presentations;
    use super::request::{build_base_context, ensure_tool_call_output_pairs};
    use super::tool_state::apply_tool_state_changes;
    use crate::config::McpServerConfig;
    use crate::tools::catalog::{MountedToolDefinition, MountedToolSourceSession};
    use crate::tools::{MountedToolSourceSessions, ToolCatalogStateChange, ToolPresentation};
    use serde_json::json;

    #[test]
    fn stitches_missing_function_call_before_output() {
        let continuation = vec![json!({
            "type": "function_call_output",
            "call_id": "call_1",
            "output": "{\"ok\":true}"
        })];
        let executed_call_items = vec![json!({
            "type": "function_call",
            "call_id": "call_1",
            "name": "tool_a",
            "arguments": "{}"
        })];

        let stitched = ensure_tool_call_output_pairs(continuation, &executed_call_items);
        assert_eq!(stitched.len(), 2);
        assert_eq!(
            stitched[0].get("type").and_then(|v| v.as_str()),
            Some("function_call")
        );
        assert_eq!(
            stitched[1].get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
    }

    #[test]
    fn strips_leading_commentary_lines_from_final_answer() {
        let cleaned = strip_commentary_prefixes(
            "Checking files.\nNow I will run tests.\nApplied the fix.",
            &[
                "Checking files.".to_string(),
                "Now I will run tests.".to_string(),
            ],
        );
        assert_eq!(cleaned, "Applied the fix.");
    }

    #[test]
    fn selects_unknown_phase_message_as_final_output_when_no_final_phase_exists() {
        let final_text = select_final_output_text(
            &[
                (
                    "Still checking.".to_string(),
                    Some(crate::message_phase::AssistantPhase::Commentary),
                ),
                ("Applied the fix.".to_string(), None),
            ],
            "Still checking.\nApplied the fix.",
            &["Still checking.".to_string()],
        );
        assert_eq!(final_text, "Applied the fix.");
    }

    #[test]
    fn merges_missing_tool_presentation_fields_from_snapshot() {
        let merged = merge_tool_presentations(
            Some(ToolPresentation {
                display_name: "Calculator".to_string(),
                description: String::new(),
                icon: None,
            }),
            Some(ToolPresentation {
                display_name: "Fallback Calculator".to_string(),
                description: "Performs math".to_string(),
                icon: Some("Calculator".to_string()),
            }),
        )
        .expect("merged tool presentation");

        assert_eq!(merged.display_name, "Calculator");
        assert_eq!(merged.description, "Performs math");
        assert_eq!(merged.icon.as_deref(), Some("Calculator"));
    }

    #[test]
    fn base_context_places_system_blocks_before_recovery_and_history() {
        let base_context = build_base_context(
            vec![json!({"role":"system","content":[{"type":"input_text","text":"sys"}]})],
            Some(json!({"role":"system","content":[{"type":"input_text","text":"recovery"}]})),
            vec![json!({"role":"user","content":[{"type":"input_text","text":"history"}]})],
            json!({"role":"user","content":[{"type":"input_text","text":"current"}]}),
        );

        assert_eq!(
            base_context
                .first()
                .and_then(|item| item.get("content"))
                .and_then(|value| value.as_array())
                .and_then(|items| items.first())
                .and_then(|item| item.get("text"))
                .and_then(|value| value.as_str()),
            Some("sys")
        );
        assert_eq!(
            base_context[1]["content"][0]["text"].as_str(),
            Some("recovery")
        );
        assert_eq!(
            base_context[2]["content"][0]["text"].as_str(),
            Some("history")
        );
        assert_eq!(
            base_context[3]["content"][0]["text"].as_str(),
            Some("current")
        );
    }

    #[test]
    fn applies_mounted_mcp_server_state_changes_between_hops() {
        let mut mounted_servers = MountedToolSourceSessions::new();
        apply_tool_state_changes(
            &mut mounted_servers,
            vec![ToolCatalogStateChange::MountToolSource(Box::new(
                MountedToolSourceSession {
                    source_id: "openai_docs".to_string(),
                    source_type: "mcp".to_string(),
                    tools: vec![MountedToolDefinition {
                        tool_name: "search_openai_docs".to_string(),
                        display_name: "Search Openai Docs".to_string(),
                        description: "Search docs".to_string(),
                        icon: Some("LayoutGrid".to_string()),
                        input_schema: json!({"type":"object","properties":{}}),
                    }],
                    mcp_config: Some(McpServerConfig::default()),
                },
            ))],
        );

        let mounted = mounted_servers
            .get("openai_docs")
            .expect("mounted server should exist");
        assert_eq!(mounted.tools.len(), 1);
        assert_eq!(mounted.tools[0].tool_name, "search_openai_docs");
    }

    #[test]
    fn removes_mounted_mcp_server_after_unmount_state_change() {
        let mut mounted_servers = MountedToolSourceSessions::new();
        mounted_servers.insert(
            "openai_docs".to_string(),
            MountedToolSourceSession {
                source_id: "openai_docs".to_string(),
                source_type: "mcp".to_string(),
                tools: vec![MountedToolDefinition {
                    tool_name: "search_openai_docs".to_string(),
                    display_name: "Search Openai Docs".to_string(),
                    description: "Search docs".to_string(),
                    icon: Some("LayoutGrid".to_string()),
                    input_schema: json!({"type":"object","properties":{}}),
                }],
                mcp_config: Some(McpServerConfig::default()),
            },
        );

        apply_tool_state_changes(
            &mut mounted_servers,
            vec![ToolCatalogStateChange::UnmountToolSource {
                source_id: "openai_docs".to_string(),
                source_type: "mcp".to_string(),
            }],
        );

        assert!(!mounted_servers.contains_key("openai_docs"));
    }
}
