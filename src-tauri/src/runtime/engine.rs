use super::stream_collection::collect_provider_turn;
use super::tool_archiving::archive_unavailable_historical_tool_calls;
use super::tool_execution::execute_pending_tools;
use super::tool_parsing::{describe_item_shape, extract_active_tool_names};
use super::AgentRuntime;
use crate::commands::chat::ChatRequest;
use crate::config::AppConfig;
use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult};
use crate::tools::{ToolCatalog, ToolExecutionContext};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use tokio::sync::watch;

// ── Turn accumulator ──────────────────────────────────────────────────────
// Collects output across all hops within a single turn (user message → final
// assistant response).  Every hop's output items are preserved so the
// frontend can reconstruct the full tool-call timeline.

struct TurnAccumulator {
    output_text: String,
    last_response_id: String,
    output_items: Vec<Value>,
    timeline_events: Vec<Value>,
}

impl TurnAccumulator {
    fn new() -> Self {
        Self {
            output_text: String::new(),
            last_response_id: String::new(),
            output_items: Vec::new(),
            timeline_events: Vec::new(),
        }
    }

    fn absorb_response(&mut self, response: &ResponseStreamResult) {
        if !response.response_id.is_empty() {
            self.last_response_id = response.response_id.clone();
        }

        if !response.output_text.is_empty() {
            if !self.output_text.is_empty() {
                self.output_text.push('\n');
            }
            self.output_text.push_str(&response.output_text);
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

// ── Request builder ───────────────────────────────────────────────────────

fn build_request(
    req: &ChatRequest,
    input_items: Vec<Value>,
    tools_schemas: Vec<Value>,
    previous_response_id: Option<&str>,
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
        previous_response_id: previous_response_id.map(ToOwned::to_owned),
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

        let tools_schemas = tools_catalog
            .list_schemas_with_format(
                tool_schema_format,
                &ToolExecutionContext {
                    conversation_id: Some(conversation_id.to_string()),
                },
            )
            .await;
        let active_tool_names = extract_active_tool_names(&tools_schemas);
        context_items =
            archive_unavailable_historical_tool_calls(context_items, &active_tool_names);

        let mut accumulator = TurnAccumulator::new();
        let mut accumulated_context: Vec<Value> = Vec::new();
        let mut repeated_failed_tool_signatures = std::collections::HashMap::new();
        let mut turn_idx = 0usize;
        let max_turns = 10usize;
        let mut working_started = false;

        // ── Build the initial input (history + current user message) ──────
        // This is the *base* context that every subsequent continuation
        // will carry forward so the model never loses sight of the original
        // question or prior conversation.
        let mut base_context = context_items;
        base_context.push(crate::providers::build_user_input_item(
            provider_kind,
            req.input.trim(),
        )?);

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

            let prev_response_id = if accumulator.last_response_id.is_empty() {
                None
            } else {
                Some(accumulator.last_response_id.as_str())
            };

            let stream_request =
                build_request(req, input_items, tools_schemas.clone(), prev_response_id);
            let collected = collect_provider_turn(
                config,
                provider_kind,
                &resolved_model.provider_key,
                &stream_request,
                cancel_rx,
                &mut on_event,
            )
            .await?;

            accumulator.absorb_response(&collected.response_result);

            // ── No tools → final response reached ─────────────────────────
            if collected.pending_tools.is_empty() {
                // Emit WorkingDone if we entered a work phase and are now done.
                if working_started {
                    on_event(ProviderStreamEvent::WorkingDone)?;
                }
                break;
            }

            // ── Emit WorkingStarted on the first tool hop ─────────────────
            if !working_started {
                working_started = true;
                on_event(ProviderStreamEvent::WorkingStarted)?;
            }

            // ── Execute pending tools locally ─────────────────────────────
            let executed_batch = execute_pending_tools(
                provider_kind,
                conversation_id,
                tools_catalog,
                collected.pending_tools,
                cancel_rx,
                &mut repeated_failed_tool_signatures,
                &mut on_event,
            )
            .await?;
            accumulator
                .timeline_events
                .extend(executed_batch.timeline_events);

            // ── Build this hop's delta items ──────────────────────────────
            let hop_delta = crate::providers::compose_tool_continuation_input(
                provider_kind,
                &collected.response_result.output_items,
                executed_batch.tool_results_items,
            )?;
            let hop_delta = ensure_tool_call_output_pairs(
                hop_delta,
                &executed_batch.executed_tool_call_items,
            );

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

        let final_res = ResponseStreamResult {
            response_id: accumulator.last_response_id,
            output_text: accumulator.output_text,
            output_items: accumulator.output_items,
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
    use super::ensure_tool_call_output_pairs;
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
}
