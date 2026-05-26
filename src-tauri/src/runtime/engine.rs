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

struct TurnAccumulator {
    output_text: String,
    response_id: String,
    output_items: Vec<Value>,
    timeline_events: Vec<Value>,
}

impl TurnAccumulator {
    fn new() -> Self {
        Self {
            output_text: String::new(),
            response_id: String::new(),
            output_items: Vec::new(),
            timeline_events: Vec::new(),
        }
    }

    fn absorb_response(&mut self, response: &ResponseStreamResult) {
        if !response.response_id.is_empty() {
            self.response_id = response.response_id.clone();
        }

        if !response.output_text.is_empty() {
            if !self.output_text.is_empty() {
                self.output_text.push('\n');
            }
            self.output_text.push_str(&response.output_text);
        }

        self.output_items.extend(response.output_items.clone());
    }
}

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
    pub(super) async fn run_turn_with_engine<F>(
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
        let mut next_input_items: Option<Vec<Value>> = None;
        let mut repeated_failed_tool_signatures = std::collections::HashMap::new();
        let mut turn_idx = 0usize;
        let max_turns = 10usize;

        'turn_loop: loop {
            if turn_idx >= max_turns {
                return Err("Maximum turn execution limit reached".to_string());
            }
            turn_idx += 1;

            let input_items = if let Some(continuation_items) = next_input_items.take() {
                continuation_items
            } else {
                let mut initial_items = context_items.clone();
                initial_items.push(crate::providers::build_user_input_item(
                    &resolved_model.provider.kind,
                    req.input.trim(),
                )?);
                initial_items
            };
            context_items = input_items.clone();

            let stream_request = build_request(req, input_items, tools_schemas.clone());
            let collected = collect_provider_turn(
                config,
                &resolved_model.provider.kind,
                &resolved_model.provider_key,
                &stream_request,
                cancel_rx,
                &mut on_event,
            )
            .await?;

            accumulator.absorb_response(&collected.response_result);

            if collected.pending_tools.is_empty() {
                break;
            }

            let executed_batch = execute_pending_tools(
                &resolved_model.provider.kind,
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

            let continuation_delta_items = crate::providers::compose_tool_continuation_input(
                &resolved_model.provider.kind,
                &collected.response_result.output_items,
                executed_batch.tool_results_items,
            )?;
            let continuation_delta_items = ensure_tool_call_output_pairs(
                continuation_delta_items,
                &executed_batch.executed_tool_call_items,
            );

            // Keep tool output items in persistent context so later turns preserve tool-call pairing.
            accumulator.output_items.extend(
                continuation_delta_items
                    .iter()
                    .filter(|item| {
                        matches!(
                            item.get("type").and_then(Value::as_str),
                            Some("function_call_output") | Some("custom_tool_call_output")
                        )
                    })
                    .cloned(),
            );

            let continuation_shape: Vec<String> = continuation_delta_items
                .iter()
                .map(describe_item_shape)
                .collect();
            log::debug!(
                "Tool continuation items for provider '{}': {}",
                resolved_model.provider_key,
                continuation_shape.join(", ")
            );

            context_items.extend(continuation_delta_items.clone());
            // New runtime architecture: tool-loop continuation submits only
            // model output items from this hop + tool outputs from local execution.
            next_input_items = Some(continuation_delta_items);

            if *cancel_rx.borrow() {
                break 'turn_loop;
            }
        }

        let final_res = ResponseStreamResult {
            response_id: accumulator.response_id,
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
