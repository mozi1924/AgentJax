use super::types::{ExecutedToolRecord, PreparedToolExecution};
use crate::conversation_store_utils::now_unix_ms;
use crate::provider_api::types::ProviderPendingToolCall;
use crate::runtime::tool_parsing::describe_item_shape;
use crate::runtime::{MAX_REPEATED_FAILED_SIGNATURES, MAX_TOOL_EXEC_RETRIES};
use crate::time_context::attach_tool_output_time_metadata;
use crate::tools::{ToolCatalogExecution, ToolCatalogSnapshot, ToolExecutionContext};
use serde_json::json;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Semaphore, watch};

pub(super) fn prepare_tool_execution(
    index: usize,
    pending: ProviderPendingToolCall,
    repeated_failed_tool_signatures: &HashMap<String, usize>,
) -> PreparedToolExecution {
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

    PreparedToolExecution {
        index,
        call_id,
        name,
        args,
        signature,
        repeated_fail_count,
        is_repeated_failure_guarded,
    }
}

pub(super) async fn run_prepared_tool(
    tool_snapshot: ToolCatalogSnapshot,
    conversation_id: String,
    cancel_rx: watch::Receiver<bool>,
    semaphore: Arc<Semaphore>,
    pending: PreparedToolExecution,
) -> Option<ExecutedToolRecord> {
    let permit = semaphore.acquire_owned().await.ok()?;
    let record = execute_prepared_tool(tool_snapshot, conversation_id, cancel_rx, pending).await;
    drop(permit);
    record
}

async fn execute_prepared_tool(
    tool_snapshot: ToolCatalogSnapshot,
    conversation_id: String,
    cancel_rx: watch::Receiver<bool>,
    pending: PreparedToolExecution,
) -> Option<ExecutedToolRecord> {
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

    let context = ToolExecutionContext::with_conversation_id(conversation_id);
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
                        last_error = Some(err.to_string());
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
