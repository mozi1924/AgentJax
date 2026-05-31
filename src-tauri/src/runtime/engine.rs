mod output;
mod presentations;
mod request;
mod tool_state;
mod turn;

use super::AgentRuntime;
use super::stream_collection::collect_provider_turn;
use super::tool_archiving::archive_unavailable_historical_tool_calls;
use super::tool_execution::ToolExecutionScheduler;
use super::tool_parsing::describe_item_shape;
use crate::commands::chat::ChatRequest;
use crate::config::AppConfig;
use crate::message_phase::AssistantPhase;
use crate::provider_api::types::{ProviderStreamEvent, ResponseStreamResult};
use crate::time_context::{build_temporal_context_developer_item, render_timed_message};
use crate::tools::{ToolCatalog, ToolExecutionContext};
use output::{
    extract_assistant_messages_from_items, resolve_hop_phase, select_final_output_text,
    strip_commentary_prefixes,
};
use presentations::enrich_tool_stream_event;
use request::{build_base_context, build_request, ensure_tool_call_output_pairs};
use serde_json::Value;
use tokio::sync::watch;
use tool_state::apply_tool_state_changes;
use turn::TurnAccumulator;

impl AgentRuntime {
    pub async fn run_turn<F>(
        config: &AppConfig,
        req: &ChatRequest,
        conversation_id: &str,
        user_message_ts: i64,
        mut context_items: Vec<Value>,
        recovery_note: Option<Value>,
        tools_catalog: &ToolCatalog,
        cancel_rx: &mut watch::Receiver<bool>,
        mut on_event: F,
    ) -> Result<(ResponseStreamResult, Vec<Value>), String>
    where
        F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send + 'static,
    {
        let resolved_model = config.resolve_model_profile(req.model.as_deref())?;
        let provider_capabilities =
            crate::provider_api::get_capabilities(&resolved_model.provider.kind)?;
        let tool_schema_format =
            crate::provider_api::get_tool_schema_format(&resolved_model.provider.kind)?;
        let provider_kind = &resolved_model.provider.kind;
        let mut developer_items = resolved_model.prompt_assembly.developer_items.clone();
        let request_started_at_unix_ms = crate::conversation_store_utils::now_unix_ms();
        developer_items.push(build_temporal_context_developer_item(
            request_started_at_unix_ms,
            user_message_ts,
        ));

        let tool_context = ToolExecutionContext {
            conversation_id: Some(conversation_id.to_string()),
        };
        let mut mounted_mcp_servers = tools_catalog.load_persisted_mounted_servers(&tool_context);
        let initial_snapshot = tools_catalog
            .snapshot_with_format_and_mounted_servers(
                tool_schema_format,
                &tool_context,
                &mounted_mcp_servers,
            )
            .await;
        context_items = archive_unavailable_historical_tool_calls(
            context_items,
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
        let max_turns = 10usize;

        // ── Build the initial input (history + current user message) ──────
        // This is the *base* context that every subsequent continuation
        // will carry forward so the model never loses sight of the original
        // question or prior conversation.
        let base_context = build_base_context(
            std::mem::take(&mut developer_items),
            recovery_note,
            std::mem::take(&mut context_items),
            crate::provider_api::build_user_input_item(
                provider_kind,
                &render_timed_message("Current user message", user_message_ts, req.input.trim()),
            )?,
        );

        'turn_loop: loop {
            if turn_idx >= max_turns {
                return Err("Maximum turn execution limit reached".to_string());
            }
            turn_idx += 1;

            // ── Determine input for this hop ──────────────────────────────
            // Hop 1: base context (history + user message).
            // Hop N: full accumulated context (base + all prior hop deltas).
            let input_items = if turn_idx == 1 {
                base_context.clone()
            } else {
                accumulated_context.clone()
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
            let stream_request = build_request(req, input_items, tool_snapshot.schemas().to_vec());
            let mut tool_scheduler = ToolExecutionScheduler::new(
                conversation_id,
                tool_snapshot.clone(),
                provider_capabilities.supports_parallel_tool_calls,
                cancel_rx,
            );
            let collected = collect_provider_turn(
                config,
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
                    if resolved_phase != Some(AssistantPhase::Commentary) {
                        final_output_text = emitted_text;
                    } else {
                        commentary_history.push(text);
                    }
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
            accumulator
                .timeline_events
                .extend(executed_batch.timeline_events);
            apply_tool_state_changes(&mut mounted_mcp_servers, executed_batch.state_changes);
            if let Err(err) =
                tools_catalog.persist_mounted_servers(conversation_id, &mounted_mcp_servers)
            {
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
            // context (base + all prior deltas), not just this hop's delta.
            // Without this the model loses all prior context after a tool
            // call, which was the root cause of "forgetting what it was
            // doing."
            if turn_idx == 1 {
                // First accumulation: seed with base context.
                accumulated_context = base_context.clone();
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
    use crate::tools::{
        MountedToolDefinition, MountedToolSourceSession, MountedToolSourceSessions,
        ToolCatalogStateChange, ToolPresentation,
    };
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
    fn base_context_places_developer_blocks_before_recovery_and_history() {
        let base_context = build_base_context(
            vec![json!({"role":"developer","content":[{"type":"input_text","text":"dev"}]})],
            Some(json!({"role":"developer","content":[{"type":"input_text","text":"recovery"}]})),
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
            Some("dev")
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
            vec![ToolCatalogStateChange::MountToolSource(
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
            )],
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
