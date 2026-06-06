use super::ToolSnapshotEntry;
use crate::agentjax_err;
use crate::error::AgentJaxResult;
use crate::tools::ToolExecutionContext;
use serde_json::{Value, json};
use std::sync::Arc;

/// Read the target tool name from a background-tool control call.
pub(super) fn background_tool_name(arguments: &Value) -> AgentJaxResult<String> {
    arguments
        .get("toolName")
        .or_else(|| arguments.get("tool_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            agentjax_err!(
                "background_task action='start' requires a non-empty toolName",
                ToolExecution
            )
        })
}

pub(super) fn background_tool_arguments(arguments: &Value) -> Value {
    arguments
        .get("arguments")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

pub(super) fn background_job_id(arguments: &Value) -> AgentJaxResult<String> {
    arguments
        .get("jobId")
        .or_else(|| arguments.get("job_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| {
            agentjax_err!(
                "background_task action='wait' requires a non-empty jobId",
                ToolExecution
            )
        })
}

pub(super) fn background_wait_timeout_ms(arguments: &Value) -> Option<u64> {
    arguments
        .get("timeoutMs")
        .or_else(|| arguments.get("timeout_ms"))
        .and_then(Value::as_u64)
}

pub(super) fn is_backgroundable_entry(entry: &ToolSnapshotEntry) -> bool {
    matches!(
        entry,
        ToolSnapshotEntry::Native(_) | ToolSnapshotEntry::Mcp { .. }
    )
}

/// Execute one snapshot entry inside a background job worker.
///
/// Delegates to the shared `execute_entry_output` in the parent snapshot
/// module, which keeps the Native/MCP dispatch logic in one place.
pub(super) async fn execute_backgroundable_entry(
    entry: ToolSnapshotEntry,
    arguments: Value,
    context: ToolExecutionContext,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_runtime: crate::config::McpRuntimeConfig,
) -> crate::error::AgentJaxResult<Value> {
    // The entry was already validated as backgroundable by `is_backgroundable_entry`.
    // Both Native and Mcp variants are handled by the shared dispatch.
    super::snapshot::execute_entry_output(&entry, &arguments, &context, &mcp_manager, &mcp_runtime)
        .await
}
