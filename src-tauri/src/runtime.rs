use crate::config::AppConfig;
use crate::providers::types::{
    ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};
use crate::tools::{ToolCatalog, ToolExecutionContext};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::time::Instant;
use tokio::sync::watch;

pub struct AgentRuntime;
const MAX_TOOL_EXEC_RETRIES: usize = 2;
const MAX_REPEATED_FAILED_SIGNATURES: usize = 3;

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

fn is_valid_pending_tool_call(call: &ProviderPendingToolCall) -> bool {
    !call.call_id.trim().is_empty() && !call.name.trim().is_empty()
}

fn parse_tool_call_item_arguments(item: &Value) -> Value {
    let Some(arguments) = item.get("arguments") else {
        return json!({});
    };

    match arguments {
        Value::Object(_) => arguments.clone(),
        Value::String(raw) => serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    }
}

fn extract_active_tool_names(tools_schemas: &[Value]) -> HashSet<String> {
    let mut names = HashSet::new();
    for schema in tools_schemas {
        if let Some(name) = schema.get("name").and_then(Value::as_str) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                names.insert(trimmed.to_string());
            }
            continue;
        }

        if let Some(name) = schema
            .get("function")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
        {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                names.insert(trimmed.to_string());
            }
        }
    }
    names
}

fn to_compact_json(value: &Value, max_chars: usize) -> String {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    if serialized.chars().count() <= max_chars {
        return serialized;
    }

    let mut out = String::new();
    for (idx, ch) in serialized.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push_str("...<truncated>");
    out
}

fn build_archived_tool_note(
    call_id: &str,
    tool_name: &str,
    arguments: &Value,
    outputs: &[Value],
) -> Value {
    let output_value = if outputs.is_empty() {
        Value::Null
    } else if outputs.len() == 1 {
        outputs[0].clone()
    } else {
        Value::Array(outputs.to_vec())
    };

    let note = format!(
        "ARCHIVED_TOOL_CALL {{\"reason\":\"tool_unavailable\",\"call_id\":\"{}\",\"tool\":\"{}\",\"arguments\":{},\"output\":{}}}\nThis tool existed in earlier turns but is currently unavailable. Keep this as historical context and do not attempt to call it unless the tool appears again in the current tool list.",
        call_id,
        tool_name,
        to_compact_json(arguments, 800),
        to_compact_json(&output_value, 1200),
    );

    json!({
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": note
        }]
    })
}

fn archive_unavailable_historical_tool_calls(
    input_items: Vec<Value>,
    active_tool_names: &HashSet<String>,
) -> Vec<Value> {
    if active_tool_names.is_empty() {
        return input_items;
    }

    let mut unavailable_calls: HashMap<String, (String, Value)> = HashMap::new();
    let mut outputs_by_call_id: HashMap<String, Vec<Value>> = HashMap::new();

    for item in &input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(
            item_type,
            "function_call_output" | "custom_tool_call_output"
        ) {
            if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                outputs_by_call_id
                    .entry(call_id.to_string())
                    .or_default()
                    .push(item.clone());
            }
            continue;
        }

        if !matches!(item_type, "function_call" | "custom_tool_call") {
            continue;
        }

        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        if active_tool_names.contains(name) {
            continue;
        }

        unavailable_calls
            .entry(call_id.to_string())
            .or_insert_with(|| (name.to_string(), parse_tool_call_item_arguments(item)));
    }

    if unavailable_calls.is_empty() {
        return input_items;
    }

    let mut emitted_call_ids = HashSet::new();
    let mut output = Vec::with_capacity(input_items.len());

    for item in input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(
            item_type,
            "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
        ) {
            output.push(item);
            continue;
        }

        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
        if call_id.is_empty() {
            output.push(item);
            continue;
        }

        if let Some((tool_name, arguments)) = unavailable_calls.get(call_id) {
            if matches!(item_type, "function_call" | "custom_tool_call")
                && !emitted_call_ids.contains(call_id)
            {
                let outputs = outputs_by_call_id.get(call_id).cloned().unwrap_or_default();
                output.push(build_archived_tool_note(
                    call_id, tool_name, arguments, &outputs,
                ));
                emitted_call_ids.insert(call_id.to_string());
            }
            continue;
        }

        output.push(item);
    }

    output
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
        let mut repeated_failed_tool_signatures: HashMap<String, usize> = HashMap::new();

        // 1. Initial tools schema mapping (provider-specific conversion)
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

        let mut turn_idx = 0;
        let max_turns = 10;

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
            let event_pending_total = pending_tools_from_events.len();
            let mut pending_tools: Vec<ProviderPendingToolCall> = pending_tools_from_events
                .into_iter()
                .filter(is_valid_pending_tool_call)
                .collect();
            let has_invalid_event_pending = pending_tools.len() != event_pending_total;

            if pending_tools.is_empty() || has_invalid_event_pending {
                let extracted_pending = crate::providers::extract_pending_tool_calls(
                    &resolved_model.provider.kind,
                    &response_result.output_items,
                )?;
                if has_invalid_event_pending {
                    log::warn!(
                        "Provider '{}' emitted incomplete tool-call events; merged fallback extraction from output items",
                        resolved_model.provider_key
                    );
                }
                if pending_tools.is_empty() && !extracted_pending.is_empty() {
                    log::debug!(
                        "Tool-call fallback path used for provider '{}': extracted {} calls from output items",
                        resolved_model.provider_key,
                        extracted_pending.len()
                    );
                }
                for extracted in extracted_pending {
                    if !is_valid_pending_tool_call(&extracted) {
                        continue;
                    }
                    if pending_tools
                        .iter()
                        .any(|existing| existing.call_id == extracted.call_id)
                    {
                        continue;
                    }
                    pending_tools.push(extracted);
                }
            }

            if pending_tools.is_empty() {
                break;
            }

            let mut tool_results_items = Vec::new();

            for pending in pending_tools {
                if *cancel_rx.borrow() {
                    break 'turn_loop;
                }
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
                let signature = format!(
                    "{}::{}",
                    name,
                    serde_json::to_string(&args).unwrap_or_default()
                );
                let repeated_fail_count = repeated_failed_tool_signatures
                    .get(&signature)
                    .copied()
                    .unwrap_or(0);
                let is_repeated_failure_guarded =
                    repeated_fail_count >= MAX_REPEATED_FAILED_SIGNATURES;
                let start_time = Instant::now();
                let mut last_error: Option<String> = None;
                let mut success_result: Option<Value> = None;
                let mut attempt = 0usize;
                let mut max_attempts = MAX_TOOL_EXEC_RETRIES;
                if is_repeated_failure_guarded {
                    max_attempts = 0;
                }
                while attempt < max_attempts {
                    if *cancel_rx.borrow() {
                        last_error = Some("Tool execution cancelled".to_string());
                        break;
                    }
                    attempt += 1;
                    let exec_result = tools_catalog
                        .execute(
                            &name,
                            &args,
                            &ToolExecutionContext {
                                conversation_id: Some(conversation_id.to_string()),
                            },
                        )
                        .await;
                    match exec_result {
                        Ok(res) => {
                            success_result = Some(res);
                            break;
                        }
                        Err(err) => {
                            last_error = Some(err);
                        }
                    }
                }
                let duration_ms = start_time.elapsed().as_millis() as u64;

                let (output_str, is_success) = if let Some(res) = success_result {
                    let output_payload = json!({
                        "ok": true,
                        "tool": name,
                        "result": res,
                    });
                    (
                        serde_json::to_string(&output_payload).unwrap_or_default(),
                        true,
                    )
                } else {
                    let error_message = if is_repeated_failure_guarded {
                        format!(
                            "Tool '{}' with the same arguments has failed {} times in a row. Stop retrying this exact call and adjust arguments or choose another approach.",
                            name, repeated_fail_count
                        )
                    } else {
                        last_error.unwrap_or_else(|| "Tool execution failed".to_string())
                    };
                    let output_payload = json!({
                        "ok": false,
                        "tool": name,
                        "error": {
                            "message": error_message,
                            "retriable": !is_repeated_failure_guarded,
                            "attempts": max_attempts,
                        }
                    });
                    (
                        serde_json::to_string(&output_payload).unwrap_or_default(),
                        false,
                    )
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
                if is_success {
                    repeated_failed_tool_signatures.remove(&signature);
                } else {
                    repeated_failed_tool_signatures
                        .entry(signature)
                        .and_modify(|count| *count += 1)
                        .or_insert(1);
                }

                let tool_input_item = crate::providers::build_tool_result_input_item(
                    &resolved_model.provider.kind,
                    &call_id,
                    &output_str,
                )?;
                tool_results_items.push(tool_input_item);
            }

            let continuation_delta_items = crate::providers::compose_tool_continuation_input(
                &resolved_model.provider.kind,
                &response_result.output_items,
                tool_results_items,
            )?;
            // Persist local tool outputs in conversation context so follow-up turns
            // can keep function_call/function_call_output pairs consistent.
            final_output_items.extend(
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

            context_items.extend(continuation_delta_items);
            // Use full in-memory context for the next tool-loop hop. Sending only
            // tool continuation fragments can make stateless providers lose the
            // original user intent and respond with bare tool output.
            next_input_items = Some(context_items.clone());

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
    use super::{
        archive_unavailable_historical_tool_calls, extract_active_tool_names, AgentRuntime,
    };
    use crate::commands::chat::ChatRequest;
    use crate::providers::types::ProviderStreamEvent;
    use crate::tools::ToolCatalog;
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;
    use tokio::sync::watch;
    use uuid::Uuid;

    #[tokio::test]
    #[ignore = "requires a real provider credential and network access"]
    async fn real_gateway_tool_loop_smoke_test_from_local_config() {
        if std::env::var("AGENTJAX_REAL_GATEWAY_TEST").ok().as_deref() != Some("1") {
            eprintln!("Skip real gateway smoke test. Set AGENTJAX_REAL_GATEWAY_TEST=1 to enable.");
            return;
        }

        let config = crate::config::load_config().expect("load local config");
        let resolved_model = config
            .resolve_model_profile(None)
            .expect("resolve default model profile");
        assert!(
            resolved_model.provider.resolved_credential().is_some(),
            "Active/default provider has no resolved credential. Check config.yaml credential or credential_env."
        );

        let conversation_id = format!("test-real-gateway-{}", Uuid::new_v4());
        crate::conversation_store::ensure_conversation(
            &conversation_id,
            config.utility_small_model_key(),
        )
        .expect("ensure conversation workspace");

        let tools_catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
        let req = ChatRequest {
            input: "请先调用 get_system_time 工具获取系统时间，然后用中文给出一句简短结论，并包含“链路测试通过”这六个字。".to_string(),
            conversation_id: Some(conversation_id.clone()),
            model: Some(config.default_model.clone()),
            reasoning_effort: None,
            request_id: Some(format!("req-real-gateway-{}", Uuid::new_v4())),
        };

        let (_cancel_tx, mut cancel_rx) = watch::channel(false);
        let stream_events: Arc<Mutex<Vec<ProviderStreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let stream_events_for_closure = stream_events.clone();

        let run_result = tokio::time::timeout(
            Duration::from_secs(180),
            AgentRuntime::run_turn(
                &config,
                &req,
                &conversation_id,
                Vec::new(),
                &tools_catalog,
                &mut cancel_rx,
                move |event| {
                    stream_events_for_closure
                        .lock()
                        .expect("lock stream events")
                        .push(event.clone());
                    Ok(())
                },
            ),
        )
        .await
        .expect("real gateway run_turn timed out")
        .expect("run_turn failed");

        let (response, timeline_events) = run_result;
        assert!(
            !response.output_text.trim().is_empty(),
            "Assistant output should not be empty"
        );

        let has_system_time_tool = timeline_events.iter().any(|event| {
            event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
                && event.get("name").and_then(|v| v.as_str()) == Some("get_system_time")
        });
        assert!(
            has_system_time_tool,
            "Expected get_system_time tool call in timeline events"
        );

        let has_tool_executed_event = stream_events
            .lock()
            .expect("lock stream events for assert")
            .iter()
            .any(|event| matches!(event, ProviderStreamEvent::ToolCallExecuted { .. }));
        assert!(
            has_tool_executed_event,
            "Expected ToolCallExecuted event in provider stream"
        );

        assert!(
            response.output_text.contains("链路测试通过"),
            "Assistant output should include verification phrase. Actual: {}",
            response.output_text
        );
    }

    #[test]
    fn archives_unavailable_tool_call_pairs_into_developer_note() {
        let active_tools = extract_active_tool_names(&[json!({
            "type": "function",
            "name": "calculator",
            "description": "",
            "parameters": {"type":"object"}
        })]);

        let context = vec![
            json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}),
            json!({"type":"function_call","call_id":"call_old","name":"mcp__github__search_repos","arguments":"{\"q\":\"agent\"}"}),
            json!({"type":"function_call_output","call_id":"call_old","output":"{\"ok\":true,\"result\":[1,2]}"}),
            json!({"type":"function_call","call_id":"call_keep","name":"calculator","arguments":"{\"expression\":\"1+1\"}"}),
            json!({"type":"function_call_output","call_id":"call_keep","output":"{\"ok\":true,\"result\":2}"}),
        ];

        let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);
        assert!(
            normalized.iter().any(|item| {
                item.get("role").and_then(|v| v.as_str()) == Some("developer")
                    && item
                        .get("content")
                        .and_then(|v| v.as_array())
                        .and_then(|arr| arr.first())
                        .and_then(|part| part.get("text"))
                        .and_then(|v| v.as_str())
                        .map(|text| text.contains("ARCHIVED_TOOL_CALL"))
                        .unwrap_or(false)
            }),
            "expected a developer archived-tool note"
        );
        assert!(
            !normalized.iter().any(|item| {
                item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                    && item.get("call_id").and_then(|v| v.as_str()) == Some("call_old")
            }),
            "unavailable historical function_call should be removed from executable context items"
        );
        assert!(
            normalized.iter().any(|item| {
                item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                    && item.get("call_id").and_then(|v| v.as_str()) == Some("call_keep")
            }),
            "available tool call should be preserved"
        );
    }
}
