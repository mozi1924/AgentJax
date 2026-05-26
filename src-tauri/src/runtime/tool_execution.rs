use super::tool_parsing::describe_item_shape;
use super::{MAX_REPEATED_FAILED_SIGNATURES, MAX_TOOL_EXEC_RETRIES};
use crate::providers::types::{ProviderPendingToolCall, ProviderStreamEvent};
use crate::tools::{ToolCatalog, ToolExecutionContext};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::Instant;
use tokio::sync::watch;

pub(super) struct ExecutedToolBatch {
    pub tool_results_items: Vec<Value>,
    pub executed_tool_call_items: Vec<Value>,
    pub timeline_events: Vec<Value>,
}

pub(super) async fn execute_pending_tools<F>(
    provider_kind: &str,
    conversation_id: &str,
    tools_catalog: &ToolCatalog,
    pending_tools: Vec<ProviderPendingToolCall>,
    cancel_rx: &mut watch::Receiver<bool>,
    repeated_failed_tool_signatures: &mut HashMap<String, usize>,
    on_event: &mut F,
) -> Result<ExecutedToolBatch, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    let mut tool_results_items = Vec::new();
    let mut executed_tool_call_items = Vec::new();
    let mut timeline_events = Vec::new();

    for pending in pending_tools {
        if *cancel_rx.borrow() {
            break;
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
        let is_repeated_failure_guarded = repeated_fail_count >= MAX_REPEATED_FAILED_SIGNATURES;

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
            name: name.clone(),
            output: output_str.clone(),
        })?;

        let output_val: Value =
            serde_json::from_str(&output_str).unwrap_or_else(|_| Value::String(output_str.clone()));
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

        let tool_input_item =
            crate::providers::build_tool_result_input_item(provider_kind, &call_id, &output_str)?;
        tool_results_items.push(tool_input_item);

        executed_tool_call_items.push(json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string()),
        }));
    }

    Ok(ExecutedToolBatch {
        tool_results_items,
        executed_tool_call_items,
        timeline_events,
    })
}
