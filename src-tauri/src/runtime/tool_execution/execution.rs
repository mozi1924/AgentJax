use super::types::{ExecutedToolRecord, PreparedToolExecution};
use crate::conversation_store_utils::now_unix_ms;
use crate::provider_api::types::ProviderPendingToolCall;
use crate::runtime::tool_parsing::describe_item_shape;
use crate::time_context::attach_tool_output_time_metadata;
use crate::tools::{ToolCatalogSnapshot, ToolExecutionContext};
use serde_json::json;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::{Semaphore, watch};

pub(super) fn prepare_tool_execution(
    index: usize,
    pending: ProviderPendingToolCall,
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

    PreparedToolExecution {
        index,
        call_id,
        name,
        args,
    }
}

pub(super) async fn run_prepared_tool(
    tool_snapshot: ToolCatalogSnapshot,
    tool_context: ToolExecutionContext,
    cancel_rx: watch::Receiver<bool>,
    semaphore: Arc<Semaphore>,
    pending: PreparedToolExecution,
) -> Option<ExecutedToolRecord> {
    let permit = semaphore.acquire_owned().await.ok()?;
    let record = execute_prepared_tool(tool_snapshot, tool_context, cancel_rx, pending).await;
    drop(permit);
    record
}

async fn execute_prepared_tool(
    tool_snapshot: ToolCatalogSnapshot,
    tool_context: ToolExecutionContext,
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
    } = pending;

    let context = tool_context;
    let start_time = Instant::now();
    let started_at_unix_ms = now_unix_ms();

    // ── Single attempt — no framework-level retry. The LLM decides whether
    //     to retry based on the structured error output. ──
    let exec_future = tool_snapshot.execute_with_effects(&name, &args, &context);
    let mut cancel_changed = cancel_rx.clone();

    let exec_result = tokio::select! {
        exec_result = exec_future => exec_result,
        changed = cancel_changed.changed() => {
            if changed.is_err() || *cancel_changed.borrow() {
                return None;
            }
            // Unreachable — changed() returns Ok only when true.
            return None;
        }
    };

    let duration_ms = start_time.elapsed().as_millis() as u64;
    let completed_at_unix_ms = now_unix_ms();

    let (output_str, is_success, state_changes) = match exec_result {
        Ok(res) => {
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
        }
        Err(err) => {
            let error_message = err.to_string();
            let output_payload = json!({
                "ok": false,
                "tool": name,
                "error": {
                    "message": error_message,
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
        }
    };

    Some(ExecutedToolRecord {
        index,
        call_id,
        name,
        args,
        output_str,
        is_success,
        started_at_unix_ms,
        completed_at_unix_ms,
        duration_ms,
        state_changes,
    })
}
