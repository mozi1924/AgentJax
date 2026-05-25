use crate::config::AppConfig;
use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult};
use crate::tools::ToolCatalog;
use crate::tools::ToolSchemaFormat;
use serde_json::{json, Value};
use std::time::Instant;
use tokio::sync::watch;

pub struct AgentRuntime;

fn collect_reasoning_items(output_items: &[Value]) -> Vec<Value> {
    output_items
        .iter()
        .filter(|item| item.get("type").and_then(Value::as_str) == Some("reasoning"))
        .cloned()
        .collect()
}

fn build_function_call_output_item(call_id: &str, output: &str) -> Value {
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output
    })
}

fn describe_item_shape(item: &Value) -> String {
    if let Some(kind) = item.get("type").and_then(Value::as_str) {
        return format!("type:{kind}");
    }
    if let Some(role) = item.get("role").and_then(Value::as_str) {
        return format!("role:{role}");
    }
    "unknown".to_string()
}

impl AgentRuntime {
    pub async fn run_turn<F>(
        config: &AppConfig,
        req: &crate::commands::chat::ChatRequest,
        _conversation_id: &str,
        mut context_items: Vec<Value>,
        previous_response_id: Option<String>,
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

        let mut active_prev_response_id =
            if provider_capabilities.supports_cross_socket_continuation {
                previous_response_id
            } else {
                None
            };
        let mut final_output_text = String::new();
        let mut final_response_id = String::new();
        let mut final_output_items = Vec::new();
        let mut timeline_events = Vec::new();
        let final_capabilities = provider_capabilities;
        let mut next_input_items: Option<Vec<Value>> = None;

        // 1. Initial tools schema mapping (provider-specific conversion)
        let tools_schemas = tools_catalog
            .list_schemas_with_format(tool_schema_format)
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
                initial_items.push(json!({
                    "role": "user",
                    "content": [{
                        "type": "text",
                        "text": req.input.trim()
                    }]
                }));
                initial_items
            };

            // Build response request
            let stream_request = ResponseStreamRequest {
                input_items,
                previous_response_id: active_prev_response_id.clone(),
                model: req.model.clone(),
                reasoning_effort: req.reasoning_effort.clone(),
                instructions_override: None,
                tools: Some(tools_schemas.clone()),
                tool_choice: Some(serde_json::Value::String("auto".to_string())),
            };

            // Call provider to stream response
            let mut active_tool_calls_in_turn = std::collections::HashMap::new();

            let provider_res =
                crate::providers::stream_response(config, &stream_request, cancel_rx, |event| {
                    match &event {
                        ProviderStreamEvent::ToolCallStarted { call_id, name, .. } => {
                            active_tool_calls_in_turn
                                .insert(call_id.clone(), (name.clone(), Value::Null));
                        }
                        ProviderStreamEvent::ToolCallArgumentsDelta { .. } => {
                            // Accumulate arguments if delta is emitted
                        }
                        ProviderStreamEvent::ToolCallCompleted {
                            call_id,
                            name,
                            arguments,
                            ..
                        } => {
                            let parsed_args: Value =
                                serde_json::from_str(arguments).unwrap_or_default();
                            active_tool_calls_in_turn
                                .insert(call_id.clone(), (name.clone(), parsed_args));
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
                active_prev_response_id = Some(response_result.response_id.clone());
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

            // Check if there are tool calls that need execution (SSE/REST)
            let mut pending_tools = Vec::new();

            for item in &response_result.output_items {
                if item.get("type").and_then(Value::as_str) == Some("function_call") {
                    if let (Some(call_id), Some(name), Some(args)) = (
                        item.get("call_id").and_then(Value::as_str),
                        item.get("name").and_then(Value::as_str),
                        item.get("arguments"),
                    ) {
                        let has_output = response_result.output_items.iter().any(|other| {
                            other.get("type").and_then(Value::as_str)
                                == Some("function_call_output")
                                && other.get("call_id").and_then(Value::as_str) == Some(call_id)
                        });
                        if !has_output {
                            pending_tools.push((
                                call_id.to_string(),
                                name.to_string(),
                                args.clone(),
                            ));
                        }
                    }
                }
            }

            if pending_tools.is_empty() {
                break;
            }

            let use_native_response_items =
                matches!(tool_schema_format, ToolSchemaFormat::Responses);
            let mut tool_results_items = Vec::new();

            for (call_id, name, args) in pending_tools {
                let start_time = Instant::now();
                let exec_result = tools_catalog.execute(&name, &args).await;
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

                let tool_input_item = if use_native_response_items {
                    build_function_call_output_item(&call_id, &output_str)
                } else {
                    json!({
                        "role": "tool",
                        "tool_call_id": call_id.clone(),
                        "content": [
                            {
                                "type": "tool_output",
                                "text": output_str.clone()
                            }
                        ]
                    })
                };
                tool_results_items.push(tool_input_item);
            }

            let continuation_input_items = if use_native_response_items {
                let mut items = collect_reasoning_items(&response_result.output_items);
                items.extend(tool_results_items);
                items
            } else {
                let assistant_item = json!({
                    "role": "assistant",
                    "content": response_result.output_items
                });
                let mut items = vec![assistant_item];
                items.extend(tool_results_items);
                items
            };

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

            if !final_capabilities.supports_cross_socket_continuation {
                active_prev_response_id = None;
            }

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

#[cfg(test)]
mod tests {
    use super::{build_function_call_output_item, collect_reasoning_items};
    use serde_json::json;

    #[test]
    fn collect_reasoning_items_only_keeps_reasoning_type() {
        let output_items = vec![
            json!({"type":"reasoning","id":"r1"}),
            json!({"type":"function_call","id":"f1"}),
            json!({"type":"message","id":"m1"}),
            json!({"type":"reasoning","id":"r2"}),
        ];

        let reasoning = collect_reasoning_items(&output_items);
        assert_eq!(reasoning.len(), 2);
        assert_eq!(reasoning[0].get("id").and_then(|v| v.as_str()), Some("r1"));
        assert_eq!(reasoning[1].get("id").and_then(|v| v.as_str()), Some("r2"));
    }

    #[test]
    fn function_call_output_item_shape_matches_responses_input_item() {
        let item = build_function_call_output_item("call_123", "{\"ok\":true}");
        assert_eq!(
            item.get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );
        assert_eq!(
            item.get("call_id").and_then(|v| v.as_str()),
            Some("call_123")
        );
        assert_eq!(
            item.get("output").and_then(|v| v.as_str()),
            Some("{\"ok\":true}")
        );
    }
}
