use super::tool_parsing::describe_item_shape;
use super::{MAX_REPEATED_FAILED_SIGNATURES, MAX_TOOL_EXEC_RETRIES};
use crate::conversation_store_utils::now_unix_ms;
use crate::providers::types::{ProviderPendingToolCall, ProviderStreamEvent};
use crate::time_context::attach_tool_output_time_metadata;
use crate::tools::{
    ToolCatalogExecution, ToolCatalogSnapshot, ToolCatalogStateChange, ToolExecutionContext,
};
use futures_util::stream::{self, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use tokio::sync::watch;

const MAX_PARALLEL_TOOL_EXECUTIONS: usize = 4;
const TOOL_PROGRESS_HEARTBEAT_SECS: u64 = 5;

pub(super) struct ExecutedToolBatch {
    pub tool_results_items: Vec<Value>,
    pub executed_tool_call_items: Vec<Value>,
    pub timeline_events: Vec<Value>,
    pub state_changes: Vec<ToolCatalogStateChange>,
}

struct ExecutedToolRecord {
    index: usize,
    call_id: String,
    name: String,
    args: Value,
    signature: String,
    output_str: String,
    is_success: bool,
    started_at_unix_ms: i64,
    completed_at_unix_ms: i64,
    duration_ms: u64,
    state_changes: Vec<ToolCatalogStateChange>,
}

struct PreparedToolExecution {
    index: usize,
    call_id: String,
    name: String,
    args: Value,
    signature: String,
    repeated_fail_count: usize,
    is_repeated_failure_guarded: bool,
}

struct ActiveToolExecution {
    name: String,
    started_at: Instant,
}

pub(super) async fn execute_pending_tools<F>(
    provider_kind: &str,
    conversation_id: &str,
    tool_snapshot: &ToolCatalogSnapshot,
    pending_tools: Vec<ProviderPendingToolCall>,
    supports_parallel_tool_calls: bool,
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
    let mut state_changes = Vec::new();

    let parallelism = if supports_parallel_tool_calls {
        MAX_PARALLEL_TOOL_EXECUTIONS.max(1)
    } else {
        1
    };

    let prepared_tools: Vec<PreparedToolExecution> = pending_tools
        .into_iter()
        .enumerate()
        .map(|(index, pending)| {
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

            PreparedToolExecution {
                index,
                call_id,
                name,
                args,
                signature,
                repeated_fail_count,
                is_repeated_failure_guarded,
            }
        })
        .collect();

    let mut active_tools: HashMap<String, ActiveToolExecution> = prepared_tools
        .iter()
        .map(|pending| {
            (
                pending.call_id.clone(),
                ActiveToolExecution {
                    name: pending.name.clone(),
                    started_at: Instant::now(),
                },
            )
        })
        .collect();

    let execution_stream = stream::iter(prepared_tools.into_iter()).map(|pending| {
        let cancel_rx = cancel_rx.clone();
        let context = ToolExecutionContext {
            conversation_id: Some(conversation_id.to_string()),
        };
        async move {
            if *cancel_rx.borrow() {
                return None;
            }

            let PreparedToolExecution {
                index,
                call_id,
                name,
                args,
                signature,
                repeated_fail_count,
                is_repeated_failure_guarded,
            } = pending;

            let start_time = Instant::now();
            let started_at_unix_ms = now_unix_ms();
            let mut last_error: Option<String> = None;
            let mut success_result: Option<ToolCatalogExecution> = None;
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
                let exec_future = tool_snapshot.execute_with_effects(&name, &args, &context);
                let mut cancel_changed = cancel_rx.clone();
                tokio::select! {
                    exec_result = exec_future => {
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
                    changed = cancel_changed.changed() => {
                        // Dropping the execution future is the fastest local
                        // cancellation path for slow MCP/network-backed tools.
                        if changed.is_err() || *cancel_changed.borrow() {
                            last_error = Some("Tool execution cancelled".to_string());
                            break;
                        }
                    }
                }
            }
            let duration_ms = start_time.elapsed().as_millis() as u64;
            let completed_at_unix_ms = now_unix_ms();

            let (output_str, is_success, state_changes) = if let Some(res) = success_result {
                let output_payload = json!({
                    "ok": true,
                    "tool": name,
                    "result": res.output,
                });
                let output_payload = attach_tool_output_time_metadata(
                    &output_payload,
                    started_at_unix_ms,
                    Some(completed_at_unix_ms),
                    Some(duration_ms),
                );
                (
                    serde_json::to_string(&output_payload).unwrap_or_default(),
                    true,
                    res.state_changes,
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
                let output_payload = attach_tool_output_time_metadata(
                    &output_payload,
                    started_at_unix_ms,
                    Some(completed_at_unix_ms),
                    Some(duration_ms),
                );
                (
                    serde_json::to_string(&output_payload).unwrap_or_default(),
                    false,
                    Vec::new(),
                )
            };

            Some(ExecutedToolRecord {
                index,
                call_id,
                name,
                args,
                signature,
                output_str,
                is_success,
                started_at_unix_ms,
                completed_at_unix_ms,
                duration_ms,
                state_changes,
            })
        }
    });

    let execution_stream = execution_stream
        .buffer_unordered(parallelism)
        .filter_map(async move |record| record);
    futures_util::pin_mut!(execution_stream);

    // Emit lightweight heartbeats while tools are still running. This keeps the
    // session/UI lifecycle explicit for long MCP calls without changing the
    // provider continuation contract: the model still receives results only
    // after the local tool batch has produced them.
    let mut progress_interval =
        tokio::time::interval(Duration::from_secs(TOOL_PROGRESS_HEARTBEAT_SECS));
    progress_interval.tick().await;

    let mut executed_records: Vec<ExecutedToolRecord> = Vec::new();
    while !active_tools.is_empty() {
        tokio::select! {
            maybe_record = execution_stream.next() => {
                let Some(record) = maybe_record else {
                    break;
                };
                active_tools.remove(&record.call_id);

                on_event(ProviderStreamEvent::ToolCallExecuted {
                    call_id: record.call_id.clone(),
                    name: record.name.clone(),
                    output: record.output_str.clone(),
                    is_success: record.is_success,
                    started_at_unix_ms: record.started_at_unix_ms,
                    completed_at_unix_ms: record.completed_at_unix_ms,
                    duration_ms: record.duration_ms,
                    presentation: tool_snapshot.presentation_for(&record.name).cloned(),
                })?;
                executed_records.push(record);
            }
            _ = progress_interval.tick() => {
                for (call_id, active) in &active_tools {
                    on_event(ProviderStreamEvent::ToolCallProgress {
                        call_id: call_id.clone(),
                        name: active.name.clone(),
                        elapsed_ms: active.started_at.elapsed().as_millis() as u64,
                        presentation: tool_snapshot.presentation_for(&active.name).cloned(),
                    })?;
                }
            }
        }
    }

    executed_records.sort_by_key(|record| record.index);

    for record in executed_records {
        let ExecutedToolRecord {
            call_id,
            name,
            args,
            signature,
            output_str,
            is_success,
            started_at_unix_ms,
            completed_at_unix_ms,
            duration_ms,
            state_changes: record_state_changes,
            ..
        } = record;

        let output_val: Value =
            serde_json::from_str(&output_str).unwrap_or_else(|_| Value::String(output_str.clone()));
        timeline_events.push(json!({
            "type": "toolCall",
            "callId": call_id.clone(),
            "name": name.clone(),
            "arguments": args.clone(),
            "output": output_val,
            "status": if is_success { "success" } else { "failed" },
            "startedAtUnixMs": started_at_unix_ms,
            "completedAtUnixMs": completed_at_unix_ms,
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
        state_changes.extend(record_state_changes);

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
        state_changes,
    })
}
