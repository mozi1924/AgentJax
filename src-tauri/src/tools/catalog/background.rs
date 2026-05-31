use super::ToolSnapshotEntry;
use crate::tools::ToolExecutionContext;
use serde_json::{Value, json};
use std::sync::Arc;

/// Read the target tool name from a background-tool control call.
pub(super) fn background_tool_name(arguments: &Value) -> Result<String, String> {
    arguments
        .get("toolName")
        .or_else(|| arguments.get("tool_name"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "background_task action='start' requires a non-empty toolName".to_string())
}

pub(super) fn background_tool_arguments(arguments: &Value) -> Value {
    arguments
        .get("arguments")
        .filter(|value| value.is_object())
        .cloned()
        .unwrap_or_else(|| json!({}))
}

pub(super) fn background_job_id(arguments: &Value) -> Result<String, String> {
    arguments
        .get("jobId")
        .or_else(|| arguments.get("job_id"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| "background_task action='wait' requires a non-empty jobId".to_string())
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
pub(super) async fn execute_backgroundable_entry(
    entry: ToolSnapshotEntry,
    arguments: Value,
    context: ToolExecutionContext,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_runtime: crate::config::McpRuntimeConfig,
) -> Result<Value, String> {
    match entry {
        ToolSnapshotEntry::Native(tool) => tool.execute(&arguments, &context),
        ToolSnapshotEntry::Mcp {
            server_id,
            tool_name,
            server_config,
        } => {
            mcp_manager
                .call_tool(
                    &server_id,
                    &server_config,
                    &mcp_runtime,
                    &tool_name,
                    arguments,
                )
                .await
        }
        ToolSnapshotEntry::Plugin { .. } => {
            Err("Plugin tools are not supported as background jobs yet".to_string())
        }
        _ => Err("Only native and MCP tools can run as background jobs".to_string()),
    }
}
