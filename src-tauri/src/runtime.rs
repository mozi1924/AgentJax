use crate::config::AppConfig;
use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult};
use crate::tools::ToolCatalog;
use serde_json::{json, Value};
use std::time::Instant;
use tokio::sync::watch;

pub struct AgentRuntime;

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
        let mut active_prev_response_id = previous_response_id;
        let mut final_output_text = String::new();
        let mut final_response_id = String::new();
        let mut final_output_items = Vec::new();
        let mut timeline_events = Vec::new();
        let mut final_capabilities;

        // 1. Initial tools schema mapping
        let tools_schemas = tools_catalog.list_schemas().await;

        let mut turn_idx = 0;
        let max_turns = 10;

        loop {
            if turn_idx >= max_turns {
                return Err("Maximum turn execution limit reached".to_string());
            }
            turn_idx += 1;

            // Build response request
            let stream_request = ResponseStreamRequest {
                input_text: req.input.trim().to_string(),
                previous_response_id: active_prev_response_id.clone(),
                model: req.model.clone(),
                reasoning_effort: req.reasoning_effort.clone(),
                context_items: context_items.clone(),
                instructions_override: None,
                tools: Some(tools_schemas.clone()),
                tool_choice: Some(serde_json::Value::String("auto".to_string())),
            };

            // Call provider to stream response
            let mut active_tool_calls_in_turn = std::collections::HashMap::new();

            let provider_res = crate::providers::stream_response(
                config,
                &stream_request,
                Some(tools_catalog),
                cancel_rx,
                |event| {
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
                },
            )
            .await;

            let response_result = match provider_res {
                Ok(res) => res,
                Err(err) => {
                    return Err(err);
                }
            };

            final_capabilities = response_result.capabilities.clone();

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

            // Standard OpenAI format check
            if let Some(choices) = response_result
                .output_items
                .first()
                .and_then(|i| i.get("choices"))
                .and_then(Value::as_array)
            {
                if let Some(first) = choices.first() {
                    if let Some(message) = first.get("message") {
                        if let Some(tool_calls) =
                            message.get("tool_calls").and_then(Value::as_array)
                        {
                            for tc in tool_calls {
                                if let (Some(call_id), Some(func)) =
                                    (tc.get("id").and_then(Value::as_str), tc.get("function"))
                                {
                                    if let Some(name) = func.get("name").and_then(Value::as_str) {
                                        let arguments_str = func
                                            .get("arguments")
                                            .and_then(Value::as_str)
                                            .unwrap_or("{}");
                                        let args: Value =
                                            serde_json::from_str(arguments_str).unwrap_or_default();
                                        pending_tools.push((
                                            call_id.to_string(),
                                            name.to_string(),
                                            args,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                }
            }

            if pending_tools.is_empty() {
                break;
            }

            let mut tool_results_items = Vec::new();

            for (call_id, name, args) in pending_tools {
                on_event(ProviderStreamEvent::ToolCallStarted {
                    item_id: format!("item-{}", call_id),
                    call_id: call_id.clone(),
                    name: name.clone(),
                })?;

                let args_str = serde_json::to_string(&args).unwrap_or_default();
                on_event(ProviderStreamEvent::ToolCallCompleted {
                    item_id: format!("item-{}", call_id),
                    call_id: call_id.clone(),
                    name: name.clone(),
                    arguments: args_str,
                })?;

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

                let tool_input_item = json!({
                    "role": "tool",
                    "tool_call_id": call_id.clone(),
                    "content": [
                        {
                            "type": "tool_output",
                            "text": output_str.clone()
                        }
                    ]
                });
                tool_results_items.push(tool_input_item);
            }

            let assistant_item = json!({
                "role": "assistant",
                "content": response_result.output_items
            });
            context_items.push(assistant_item);

            for result_item in tool_results_items {
                context_items.push(result_item);
            }

            if *cancel_rx.borrow() {
                break;
            }
        }

        let resolved_model = config.resolve_model_profile(req.model.as_deref())?;

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
