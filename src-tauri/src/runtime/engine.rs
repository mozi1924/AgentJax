use super::stream_collection::collect_provider_turn;
use super::tool_archiving::archive_unavailable_historical_tool_calls;
use super::tool_execution::execute_pending_tools;
use super::tool_parsing::describe_item_shape;
use super::AgentRuntime;
use crate::commands::chat::ChatRequest;
use crate::config::AppConfig;
use crate::message_phase::AssistantPhase;
use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult};
use crate::tools::{
    MountedMcpServerSessions, ToolCatalog, ToolCatalogSnapshot, ToolCatalogStateChange,
    ToolExecutionContext, ToolPresentation,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tokio::sync::watch;

// ── Turn accumulator ──────────────────────────────────────────────────────
// Collects output items across all hops within a single turn so the
// frontend can reconstruct the full tool-call timeline.  Per-hop assistant
// text is emitted via `HopAssistantText` events as each hop completes;
// the accumulator does NOT merge text across hops.

struct TurnAccumulator {
    last_response_id: String,
    output_items: Vec<Value>,
    timeline_events: Vec<Value>,
}

impl TurnAccumulator {
    fn new() -> Self {
        Self {
            last_response_id: String::new(),
            output_items: Vec::new(),
            timeline_events: Vec::new(),
        }
    }

    fn record_hop(&mut self, response: &ResponseStreamResult) {
        if !response.response_id.is_empty() {
            self.last_response_id = response.response_id.clone();
        }
        self.output_items.extend(response.output_items.clone());
    }

    /// Append all items from a continuation batch (reasoning, function_call,
    /// function_call_output, etc.) so the final result contains the complete
    /// tool-call timeline, not just text output.
    fn absorb_continuation_batch(&mut self, items: &[Value]) {
        self.output_items.extend(items.iter().cloned());
    }
}

fn normalize_whitespace(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn strip_commentary_prefixes(final_text: &str, commentary_history: &[String]) -> String {
    if commentary_history.is_empty() {
        return final_text.trim().to_string();
    }

    let commentary_norms: Vec<String> = commentary_history
        .iter()
        .map(|text| normalize_whitespace(text))
        .filter(|text| !text.is_empty())
        .collect();
    if commentary_norms.is_empty() {
        return final_text.trim().to_string();
    }

    let mut remaining_lines: Vec<&str> = final_text.lines().collect();
    loop {
        let first_non_empty_idx = remaining_lines
            .iter()
            .position(|line| !line.trim().is_empty());
        let Some(idx) = first_non_empty_idx else {
            return final_text.trim().to_string();
        };
        let first_line = remaining_lines[idx].trim();
        let first_line_norm = normalize_whitespace(first_line);
        if commentary_norms.iter().any(|item| item == &first_line_norm) {
            remaining_lines.drain(..=idx);
            continue;
        }
        break;
    }

    remaining_lines.join("\n").trim().to_string()
}

fn extract_assistant_messages_from_items(items: &[Value]) -> Vec<(String, Option<AssistantPhase>)> {
    items
        .iter()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
        .filter_map(|item| {
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            Some((
                text,
                item.get("phase")
                    .and_then(Value::as_str)
                    .and_then(AssistantPhase::from_api_value),
            ))
        })
        .collect()
}

fn resolve_hop_phase(
    explicit_phase: Option<AssistantPhase>,
    is_final_hop: bool,
) -> Option<AssistantPhase> {
    explicit_phase.or(Some(if is_final_hop {
        AssistantPhase::FinalAnswer
    } else {
        AssistantPhase::Commentary
    }))
}

fn merge_tool_presentations(
    existing: Option<ToolPresentation>,
    fallback: Option<ToolPresentation>,
) -> Option<ToolPresentation> {
    match (existing, fallback) {
        (Some(mut existing), Some(fallback)) => {
            if existing.display_name.trim().is_empty() {
                existing.display_name = fallback.display_name;
            }
            if existing.description.trim().is_empty() {
                existing.description = fallback.description;
            }
            let icon_missing = existing
                .icon
                .as_deref()
                .map(str::trim)
                .map(|icon| icon.is_empty())
                .unwrap_or(true);
            if icon_missing {
                existing.icon = fallback.icon;
            }
            Some(existing)
        }
        (Some(existing), None) => Some(existing),
        (None, fallback) => fallback,
    }
}

fn enrich_tool_presentation(
    existing: Option<ToolPresentation>,
    snapshot: &ToolCatalogSnapshot,
    tool_name: &str,
) -> Option<ToolPresentation> {
    merge_tool_presentations(existing, snapshot.presentation_for(tool_name).cloned())
}

fn enrich_tool_stream_event(
    event: ProviderStreamEvent,
    snapshot: &ToolCatalogSnapshot,
) -> ProviderStreamEvent {
    match event {
        ProviderStreamEvent::ToolCallStarted {
            item_id,
            call_id,
            name,
            presentation,
        } => ProviderStreamEvent::ToolCallStarted {
            presentation: enrich_tool_presentation(presentation, snapshot, &name),
            item_id,
            call_id,
            name,
        },
        ProviderStreamEvent::ToolCallCompleted {
            item_id,
            call_id,
            name,
            arguments,
            presentation,
        } => ProviderStreamEvent::ToolCallCompleted {
            presentation: enrich_tool_presentation(presentation, snapshot, &name),
            item_id,
            call_id,
            name,
            arguments,
        },
        other => other,
    }
}

fn select_final_output_text(
    hop_messages: &[(String, Option<AssistantPhase>)],
    fallback_output_text: &str,
    commentary_history: &[String],
) -> String {
    let preferred = hop_messages
        .iter()
        .rev()
        .find(|(_, phase)| *phase != Some(AssistantPhase::Commentary))
        .map(|(text, _)| text.as_str())
        .unwrap_or(fallback_output_text);

    strip_commentary_prefixes(preferred, commentary_history)
}

fn build_base_context(
    developer_items: Vec<Value>,
    recovery_note: Option<Value>,
    context_items: Vec<Value>,
    current_user_item: Value,
) -> Vec<Value> {
    let mut base_context = Vec::new();
    base_context.extend(developer_items);
    if let Some(note_item) = recovery_note {
        base_context.push(note_item);
    }
    base_context.extend(context_items);
    base_context.push(current_user_item);
    base_context
}

// ── Request builder ───────────────────────────────────────────────────────

fn build_request(
    req: &ChatRequest,
    input_items: Vec<Value>,
    tools_schemas: Vec<Value>,
) -> ResponseStreamRequest {
    ResponseStreamRequest {
        input_items,
        model: req.model.clone(),
        reasoning_effort: req.reasoning_effort.clone(),
        instructions_override: None,
        text: req.text.clone(),
        include: req.include.clone(),
        service_tier: req.service_tier.clone(),
        prompt_cache_key: req.prompt_cache_key.clone(),
        client_metadata: req.client_metadata.clone(),
        generate: req.generate,
        tools: Some(tools_schemas),
        tool_choice: Some(serde_json::Value::String("auto".to_string())),
    }
}

// ── Tool-call / output pairing ────────────────────────────────────────────
// The provider may omit paired function_call items when only
// function_call_output items appear in continuation.  This helper
// re-inserts the missing call items so the API always sees complete
// call→output pairs.

fn ensure_tool_call_output_pairs(
    items: Vec<Value>,
    executed_tool_call_items: &[Value],
) -> Vec<Value> {
    let existing_call_ids: HashSet<String> = items
        .iter()
        .filter_map(|item| match item.get("type").and_then(Value::as_str) {
            Some("function_call") | Some("custom_tool_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
            _ => None,
        })
        .collect();

    let mut missing_call_by_id: HashMap<String, Value> = HashMap::new();
    for call_item in executed_tool_call_items {
        let Some(call_id) = call_item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        if existing_call_ids.contains(call_id) {
            continue;
        }
        missing_call_by_id.insert(call_id.to_string(), call_item.clone());
    }

    if missing_call_by_id.is_empty() {
        return items;
    }

    let mut stitched = Vec::with_capacity(items.len() + missing_call_by_id.len());
    let mut inserted: HashSet<String> = HashSet::new();
    for item in items {
        if matches!(
            item.get("type").and_then(Value::as_str),
            Some("function_call_output") | Some("custom_tool_call_output")
        ) {
            if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                if let Some(missing_call) = missing_call_by_id.get(call_id) {
                    if !inserted.contains(call_id) {
                        stitched.push(missing_call.clone());
                        inserted.insert(call_id.to_string());
                    }
                }
            }
        }
        stitched.push(item);
    }

    for (call_id, missing_call) in missing_call_by_id {
        if !inserted.contains(&call_id) {
            stitched.push(missing_call);
        }
    }

    stitched
}

impl AgentRuntime {
    pub async fn run_turn<F>(
        config: &AppConfig,
        req: &ChatRequest,
        conversation_id: &str,
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
            crate::providers::get_capabilities(&resolved_model.provider.kind)?;
        let tool_schema_format =
            crate::providers::get_tool_schema_format(&resolved_model.provider.kind)?;
        let provider_kind = &resolved_model.provider.kind;
        let mut developer_items = resolved_model.prompt_assembly.developer_items.clone();

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
            crate::providers::build_user_input_item(provider_kind, req.input.trim())?,
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
            let collected = collect_provider_turn(
                config,
                provider_kind,
                &resolved_model.provider_key,
                &stream_request,
                cancel_rx,
                &mut |event| on_event(enrich_tool_stream_event(event, &tool_snapshot)),
            )
            .await?;

            accumulator.record_hop(&collected.response_result);

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

            // ── Execute pending tools locally ─────────────────────────────
            let executed_batch = execute_pending_tools(
                provider_kind,
                conversation_id,
                &tool_snapshot,
                collected.pending_tools,
                provider_capabilities.supports_parallel_tool_calls,
                cancel_rx,
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
            let hop_delta = crate::providers::compose_tool_continuation_input(
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
            provider_key: resolved_model.provider_key.clone(),
            model_profile: resolved_model.profile_key.clone(),
            model_id: resolved_model.model_id.clone(),
            capabilities: provider_capabilities,
        };

        Ok((final_res, accumulator.timeline_events))
    }
}

fn apply_tool_state_changes(
    mounted_mcp_servers: &mut MountedMcpServerSessions,
    state_changes: Vec<ToolCatalogStateChange>,
) {
    for state_change in state_changes {
        match state_change {
            ToolCatalogStateChange::MountMcpServer(server_session) => {
                mounted_mcp_servers.insert(server_session.server_id.clone(), server_session);
            }
            ToolCatalogStateChange::UnmountMcpServer(server_id) => {
                mounted_mcp_servers.remove(&server_id);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        apply_tool_state_changes, build_base_context, ensure_tool_call_output_pairs,
        merge_tool_presentations, select_final_output_text, strip_commentary_prefixes,
    };
    use crate::config::McpServerConfig;
    use crate::tools::{
        MountedMcpServerSession, MountedMcpServerSessions, MountedMcpToolDefinition,
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
        let mut mounted_servers = MountedMcpServerSessions::new();
        apply_tool_state_changes(
            &mut mounted_servers,
            vec![ToolCatalogStateChange::MountMcpServer(
                MountedMcpServerSession {
                    server_id: "openai_docs".to_string(),
                    server_config: McpServerConfig::default(),
                    tools: vec![MountedMcpToolDefinition {
                        tool_name: "search_openai_docs".to_string(),
                        display_name: "Search Openai Docs".to_string(),
                        description: "Search docs".to_string(),
                        icon: Some("LayoutGrid".to_string()),
                        input_schema: json!({"type":"object","properties":{}}),
                    }],
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
        let mut mounted_servers = MountedMcpServerSessions::new();
        mounted_servers.insert(
            "openai_docs".to_string(),
            MountedMcpServerSession {
                server_id: "openai_docs".to_string(),
                server_config: McpServerConfig::default(),
                tools: vec![MountedMcpToolDefinition {
                    tool_name: "search_openai_docs".to_string(),
                    display_name: "Search Openai Docs".to_string(),
                    description: "Search docs".to_string(),
                    icon: Some("LayoutGrid".to_string()),
                    input_schema: json!({"type":"object","properties":{}}),
                }],
            },
        );

        apply_tool_state_changes(
            &mut mounted_servers,
            vec![ToolCatalogStateChange::UnmountMcpServer(
                "openai_docs".to_string(),
            )],
        );

        assert!(!mounted_servers.contains_key("openai_docs"));
    }
}
