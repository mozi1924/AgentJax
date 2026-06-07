mod context_assembly;
mod kb_preretrieval;
mod lcm_persistence;
mod output;
mod presentations;
mod request;
mod tool_state;
mod turn;
mod turn_context;

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
use crate::time_context::build_temporal_context_system_item;
use crate::tools::ToolCatalog;
use context_assembly::{build_hop1_input, build_hop_prefix};
use kb_preretrieval::build_kb_context_item;
use lcm_persistence::{persist_hop_assistant_messages, persist_tool_results};
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
use turn_context::{TurnContext, build_tool_context};

impl super::AgentRuntime {
    /// Execute a full turn: resolve the model, build system prompts, run the
    /// hop loop (up to `agent.max_tool_turns` iterations), persist messages to
    /// LCM, and return the accumulated result.
    ///
    /// This is the orchestration layer. The heavy lifting is delegated to:
    /// - `turn_context` — parameter bundling
    /// - `context_assembly` — hop input construction
    /// - `kb_preretrieval` — knowledge base search
    /// - `lcm_persistence` — LCM message persistence
    /// - `stream_collection` — provider event handling
    /// - `tool_execution` — parallel tool scheduling
    #[allow(clippy::too_many_arguments)]
    pub async fn run_turn<F>(
        config: &AppConfig,
        agent: &AgentConfig,
        agent_id: &str,
        req: &ChatRequest,
        conversation_id: &str,
        user_message_ts: i64,
        _context_items: Vec<Value>,
        recovery_note: Option<Value>,
        tools_catalog: &Arc<ToolCatalog>,
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
        // ── Resolve model & capabilities ──────────────────────────────────
        let resolved_model =
            config.resolve_model_profile_with_agent(req.model.as_deref(), agent)?;
        let provider_capabilities =
            crate::provider_api::get_capabilities(&resolved_model.provider.kind)?;
        let tool_schema_format = match resolved_model.api_protocol.as_deref() {
            Some(p) if p.trim().eq_ignore_ascii_case("chat_completions") => {
                crate::tools::ToolSchemaFormat::ChatCompletions
            }
            _ => crate::provider_api::get_tool_schema_format(&resolved_model.provider.kind)?,
        };

        // ── Build system items with temporal context ─────────────────────
        let request_started_at_unix_ms = crate::conversation_store_utils::now_unix_ms();
        let mut system_items = resolved_model.prompt_assembly.system_items.clone();
        system_items.push(build_temporal_context_system_item(
            request_started_at_unix_ms,
            user_message_ts,
        ));

        // ── Detect sub-agent context ─────────────────────────────────────
        let is_sub_agent = conversation_id.contains("/sub-agent/");
        let sub_agent_type: Option<String> = if is_sub_agent {
            conversation_id
                .split("/sub-agent/")
                .nth(1)
                .and_then(|rest| rest.split('/').next())
                .map(|s| s.to_string())
        } else {
            None
        };
        let is_memory_sub_agent = sub_agent_type.as_deref() == Some("memory");

        // ── Bundled turn context ─────────────────────────────────────────
        let tctx = TurnContext {
            config,
            agent,
            agent_id,
            req,
            conversation_id,
            user_message_ts,
            provider_kind: resolved_model.provider.kind.clone(),
            resolved_model,
            provider_capabilities,
            tool_schema_format,
            tools_catalog,
            cancel_rx,
            sub_agent_event_tx,
            is_sub_agent,
            sub_agent_type,
            is_memory_sub_agent,
            is_auto_resume,
            max_turns: agent.max_tool_turns,
            system_items,
            recovery_note,
            street_items,
        };

        let tool_context = build_tool_context(&tctx);
        let mut mounted_mcp_servers =
            tctx.tools_catalog.load_persisted_mounted_servers(&tool_context);

        // ── Seed active context ──────────────────────────────────────────
        context.rebuild(tctx.conversation_id).await.ok();

        // ── Persist user message to LCM ──────────────────────────────────
        if !tctx.is_auto_resume {
            let user_text = tctx.req.input.trim();
            let request_id = tctx.req.request_id.as_deref().unwrap_or("unknown");
            let mut user_msg = crate::lcm::types::StoredMessage::new(
                crate::lcm::types::MessageId::new(),
                tctx.conversation_id,
                crate::lcm::types::MessageRole::User,
                user_text,
                crate::lcm::types::estimate_tokens(user_text),
                tctx.user_message_ts,
                1, // seq: user message is always first
                0, // hop_index: 0 for user messages
            );
            user_msg.metadata.insert(
                "request_id".to_string(),
                Value::String(request_id.to_string()),
            );
            if let Err(e) = context.persist_message(&user_msg).await {
                log::warn!("Failed to persist user message: {e}");
            }
        }

        // ── KB pre-retrieval & hop prefix ────────────────────────────────
        let mut prefix_builder = tctx.system_items.clone();
        if let Some(kb_item) = build_kb_context_item(
            tctx.config,
            tctx.agent,
            tctx.agent_id,
            tctx.req.input.trim(),
        )
        .await
        {
            prefix_builder.push(kb_item);
        }
        let hop_prefix = build_hop_prefix(
            prefix_builder,
            tctx.recovery_note.clone(),
            tctx.street_items.clone(),
        );

        // ── State accumulators ───────────────────────────────────────────
        let mut accumulator = TurnAccumulator::new();
        let mut accumulated_context: Vec<Value> = Vec::new();
        let mut final_output_text = String::new();
        let mut commentary_history: Vec<String> = Vec::new();
        let mut turn_idx = 0usize;

        // ── Main hop loop ────────────────────────────────────────────────
        'turn_loop: loop {
            if tctx.max_turns > 0 && turn_idx >= tctx.max_turns {
                return Err(AgentJaxError::internal(
                    "Maximum turn execution limit reached",
                ));
            }
            turn_idx += 1;

            // ── Build input items for this hop ──────────────────────────
            let input_items = if turn_idx == 1 {
                build_hop1_input(
                    &hop_prefix,
                    context,
                    &accumulated_context,
                    tctx.is_auto_resume,
                    &tctx.provider_kind,
                    tctx.user_message_ts,
                    tctx.req.input.trim(),
                )
                .await?
            } else {
                accumulated_context.clone()
            };

            // ── Freeze tool visibility per hop ──────────────────────────
            let tool_snapshot = tctx
                .tools_catalog
                .snapshot_with_format_and_mounted_servers(
                    tctx.tool_schema_format,
                    &tool_context,
                    &mounted_mcp_servers,
                )
                .await;

            // ── Archive unavailable tool calls ──────────────────────────
            let input_items = archive_unavailable_historical_tool_calls(
                input_items,
                tool_snapshot.active_tool_names(),
            );

            // ── Build request & stream ───────────────────────────────────
            let stream_request =
                build_request(tctx.req, input_items.clone(), tool_snapshot.schemas().to_vec());
            let mut tool_scheduler = ToolExecutionScheduler::new(
                tool_context.clone(),
                tool_snapshot.clone(),
                tctx.provider_capabilities.supports_parallel_tool_calls,
                tctx.cancel_rx,
            );
            let collected = collect_provider_turn(
                tctx.config,
                tctx.agent,
                &tctx.provider_kind,
                &tctx.resolved_model.provider_key,
                &stream_request,
                tctx.cancel_rx,
                &mut |event| on_event(enrich_tool_stream_event(event, &tool_snapshot)),
                Some(&mut tool_scheduler),
            )
            .await
            .inspect_err(|e| {
                log::error!(
                    "Provider turn failed: conv={} hop={} kind={:?} message={}",
                    tctx.conversation_id,
                    turn_idx,
                    e.kind,
                    e.message
                );
            })?;

            // ── Record hop & emit usage ──────────────────────────────────
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

            // ── Extract & emit hop assistant text ────────────────────────
            let hop_messages =
                extract_assistant_messages_from_items(&collected.response_result.output_items);
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

            // ── Persist assistant messages to LCM ────────────────────────
            persist_hop_assistant_messages(
                context,
                tctx.conversation_id,
                tctx.req.request_id.as_deref().unwrap_or("unknown"),
                &collected.response_result.response_id,
                turn_idx,
                &hop_messages_for_lcm,
                &collected.response_result.output_items,
                &collected.response_result.output_text,
                is_final_hop,
                &final_output_text,
            )
            .await;

            // ── No tools → final response ────────────────────────────────
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

            // ── Execute tools ────────────────────────────────────────────
            tool_scheduler.schedule_pending_tools(collected.pending_tools);
            let executed_batch = tool_scheduler
                .finish(&tctx.provider_kind, &mut on_event)
                .await?;

            // ── Persist tool results to LCM ──────────────────────────────
            persist_tool_results(
                context,
                tctx.conversation_id,
                tctx.req.request_id.as_deref().unwrap_or("unknown"),
                turn_idx,
                &executed_batch.tool_results_items,
                &executed_batch.executed_tool_call_items,
            )
            .await;

            // ── Apply state changes & persist MCP mounts ─────────────────
            accumulator
                .timeline_events
                .extend(executed_batch.timeline_events);
            apply_tool_state_changes(&mut mounted_mcp_servers, executed_batch.state_changes);
            if !tctx.is_sub_agent
                && let Err(err) = tctx.tools_catalog.persist_mounted_servers(
                    tctx.agent_id,
                    tctx.conversation_id,
                    &mounted_mcp_servers,
                ) {
                    log::warn!(
                        "Failed to persist mounted MCP servers for conversation '{}': {}",
                        tctx.conversation_id,
                        err
                    );
                }

            // ── Build hop delta & accumulate ─────────────────────────────
            let hop_delta = crate::provider_api::compose_tool_continuation_input(
                &tctx.provider_kind,
                &collected.response_result.output_items,
                executed_batch.tool_results_items,
            )?;
            let hop_delta =
                ensure_tool_call_output_pairs(hop_delta, &executed_batch.executed_tool_call_items);

            let delta_shape: Vec<String> = hop_delta.iter().map(describe_item_shape).collect();
            log::debug!(
                "Tool hop {} for provider '{}': delta={} total_context_items={}",
                turn_idx,
                tctx.resolved_model.provider_key,
                delta_shape.join(", "),
                accumulated_context.len() + hop_delta.len(),
            );

            if turn_idx == 1 {
                accumulated_context = input_items.clone();
            }
            accumulated_context.extend(hop_delta.clone());
            accumulator.absorb_continuation_batch(&hop_delta);

            if *tctx.cancel_rx.borrow() {
                break 'turn_loop;
            }
        }

        // ── Build final result ───────────────────────────────────────────
        let final_res = ResponseStreamResult {
            response_id: accumulator.last_response_id,
            output_text: final_output_text,
            output_items: accumulator.output_items,
            usage: accumulator.usage,
            usage_hops: accumulator.usage_hops,
            provider_key: tctx.resolved_model.provider_key.clone(),
            model_profile: tctx.resolved_model.profile_key.clone(),
            model_id: tctx.resolved_model.model_id.clone(),
            capabilities: tctx.provider_capabilities,
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
