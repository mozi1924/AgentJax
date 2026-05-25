use crate::config::AppConfig;
use crate::providers::types::{
    ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};
use crate::tools::{ToolCatalog, ToolExecutionContext};
use serde_json::{json, Value};
use std::time::Instant;
use tokio::sync::watch;

pub struct AgentRuntime;

fn describe_item_shape(item: &Value) -> String {
    if let Some(kind) = item.get("type").and_then(Value::as_str) {
        return format!("type:{kind}");
    }
    if let Some(role) = item.get("role").and_then(Value::as_str) {
        return format!("role:{role}");
    }
    "unknown".to_string()
}

fn push_or_update_pending_tool_call(
    calls: &mut Vec<ProviderPendingToolCall>,
    call_id: String,
    name: String,
    arguments: Value,
) {
    if let Some(existing) = calls.iter_mut().find(|call| call.call_id == call_id) {
        existing.name = name;
        existing.arguments = arguments;
        return;
    }

    calls.push(ProviderPendingToolCall {
        call_id,
        name,
        arguments,
    });
}

fn parse_tool_arguments(arguments: &str, fallback_delta: Option<&str>) -> Value {
    let parse_json_object = |raw: &str| -> Option<Value> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }

        let parsed = serde_json::from_str::<Value>(trimmed).ok()?;
        if parsed.is_object() {
            Some(parsed)
        } else {
            None
        }
    };

    parse_json_object(arguments)
        .or_else(|| fallback_delta.and_then(parse_json_object))
        .unwrap_or_else(|| json!({}))
}

impl AgentRuntime {
    pub async fn run_turn<F>(
        config: &AppConfig,
        req: &crate::commands::chat::ChatRequest,
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

        let mut final_output_text = String::new();
        let mut final_response_id = String::new();
        let mut final_output_items = Vec::new();
        let mut timeline_events = Vec::new();
        let final_capabilities = provider_capabilities;
        let mut next_input_items: Option<Vec<Value>> = None;

        // 1. Initial tools schema mapping (provider-specific conversion)
        let tools_schemas = tools_catalog
            .list_schemas_with_format(
                tool_schema_format,
                &ToolExecutionContext {
                    conversation_id: Some(conversation_id.to_string()),
                },
            )
            .await;

        let mut turn_idx = 0;
        let max_turns = 10;

        loop {
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

            // Build response request
            let stream_request = ResponseStreamRequest {
                input_items,
                model: req.model.clone(),
                reasoning_effort: req.reasoning_effort.clone(),
                instructions_override: None,
                tools: Some(tools_schemas.clone()),
                tool_choice: Some(serde_json::Value::String("auto".to_string())),
            };

            // Call provider to stream response
            let mut active_tool_calls_in_turn = std::collections::HashMap::new();
            let mut tool_args_delta_by_call: std::collections::HashMap<String, String> =
                std::collections::HashMap::new();
            let mut pending_tools_from_events: Vec<ProviderPendingToolCall> = Vec::new();

            let provider_res =
                crate::providers::stream_response(config, &stream_request, cancel_rx, |event| {
                    match &event {
                        ProviderStreamEvent::ToolCallStarted { call_id, name, .. } => {
                            active_tool_calls_in_turn
                                .insert(call_id.clone(), (name.clone(), Value::Null));
                        }
                        ProviderStreamEvent::ToolCallArgumentsDelta { call_id, delta, .. } => {
                            let entry = tool_args_delta_by_call.entry(call_id.clone()).or_default();
                            entry.push_str(delta);
                        }
                        ProviderStreamEvent::ToolCallCompleted {
                            call_id,
                            name,
                            arguments,
                            ..
                        } => {
                            let parsed_args = parse_tool_arguments(
                                arguments,
                                tool_args_delta_by_call.get(call_id).map(String::as_str),
                            );
                            active_tool_calls_in_turn
                                .insert(call_id.clone(), (name.clone(), parsed_args.clone()));
                            push_or_update_pending_tool_call(
                                &mut pending_tools_from_events,
                                call_id.clone(),
                                name.clone(),
                                parsed_args,
                            );
                        }
                        ProviderStreamEvent::ToolCallExecuted { call_id, output } => {
                            if let Some((name, args)) = active_tool_calls_in_turn.get(call_id) {
                                let output_val: Value = serde_json::from_str(output)
                                    .unwrap_or_else(|_| Value::String(output.clone()));
                                timeline_events.push(json!({
                                    "type": "toolCall",
                                    "callId": call_id.clone(),
                                    "name": name.clone(),
                                    "arguments": args.clone(),
                                    "output": output_val,
                                    "status": "success",
                                    "durationMs": serde_json::Value::Null
                                }));
                            }
                        }
                        _ => {}
                    }
                    on_event(event)
                })
                .await;

            let response_result = match provider_res {
                Ok(res) => res,
                Err(err) => {
                    return Err(err);
                }
            };

            if response_result.response_id.is_empty() {
                // Empty or closed response
            } else {
                final_response_id = response_result.response_id.clone();
            }

            if !response_result.output_text.is_empty() {
                if !final_output_text.is_empty() {
                    final_output_text.push_str("\n");
                }
                final_output_text.push_str(&response_result.output_text);
            }

            for item in &response_result.output_items {
                final_output_items.push(item.clone());
            }

            // Prefer event-driven tool-call extraction, fallback to output-item scan.
            let mut pending_tools = pending_tools_from_events;
            if pending_tools.is_empty() {
                pending_tools = crate::providers::extract_pending_tool_calls(
                    &resolved_model.provider.kind,
                    &response_result.output_items,
                )?;
                if !pending_tools.is_empty() {
                    log::debug!(
                        "Tool-call fallback path used for provider '{}': extracted {} calls from output items",
                        resolved_model.provider_key,
                        pending_tools.len()
                    );
                }
            }

            if pending_tools.is_empty() {
                break;
            }

            let mut tool_results_items = Vec::new();

            for pending in pending_tools {
                let call_id = pending.call_id;
                let name = pending.name;
                let args = if pending.arguments.is_object() {
                    pending.arguments
                } else {
                    log::warn!(
                        "Tool call '{}' arguments are not an object (type={}), defaulting to empty object",
                        call_id,
                        describe_item_shape(&pending.arguments)
                    );
                    json!({})
                };
                let start_time = Instant::now();
                let exec_result = tools_catalog
                    .execute(
                        &name,
                        &args,
                        &ToolExecutionContext {
                            conversation_id: Some(conversation_id.to_string()),
                        },
                    )
                    .await;
                let duration_ms = start_time.elapsed().as_millis() as u64;

                let (output_str, is_success) = match exec_result {
                    Ok(res) => (serde_json::to_string(&res).unwrap_or_default(), true),
                    Err(err) => (err, false),
                };

                on_event(ProviderStreamEvent::ToolCallExecuted {
                    call_id: call_id.clone(),
                    output: output_str.clone(),
                })?;

                let output_val: Value = serde_json::from_str(&output_str)
                    .unwrap_or_else(|_| Value::String(output_str.clone()));
                timeline_events.push(json!({
                    "type": "toolCall",
                    "callId": call_id.clone(),
                    "name": name.clone(),
                    "arguments": args.clone(),
                    "output": output_val,
                    "status": if is_success { "success" } else { "failed" },
                    "durationMs": duration_ms
                }));

                let tool_input_item = crate::providers::build_tool_result_input_item(
                    &resolved_model.provider.kind,
                    &call_id,
                    &output_str,
                )?;
                tool_results_items.push(tool_input_item);
            }

            let continuation_input_items = crate::providers::compose_tool_continuation_input(
                &resolved_model.provider.kind,
                &response_result.output_items,
                tool_results_items,
            )?;

            let continuation_shape: Vec<String> = continuation_input_items
                .iter()
                .map(describe_item_shape)
                .collect();
            log::debug!(
                "Tool continuation items for provider '{}': {}",
                resolved_model.provider_key,
                continuation_shape.join(", ")
            );

            context_items.extend(continuation_input_items.clone());
            next_input_items = Some(continuation_input_items);

            if *cancel_rx.borrow() {
                break;
            }
        }

        let final_res = ResponseStreamResult {
            response_id: final_response_id,
            output_text: final_output_text,
            output_items: final_output_items,
            provider_key: resolved_model.provider_key.clone(),
            model_profile: resolved_model.profile_key.clone(),
            model_id: resolved_model.model_id.clone(),
            capabilities: final_capabilities,
        };

        Ok((final_res, timeline_events))
    }
}
