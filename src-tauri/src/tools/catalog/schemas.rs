use super::names::{display_name_for_server, mount_tool_name_for_server};
use super::types::MountedToolDefinition;
use crate::tools::{ToolSchemaFormat, background_jobs, format_tool_schema, humanize_tool_name};
use serde_json::{Value, json};

pub(super) const START_BACKGROUND_TOOL_NAME: &str = "start_background_tool";
pub(super) const WAIT_BACKGROUND_TOOL_NAME: &str = "wait_background_tool";
pub(super) const CANCEL_BACKGROUND_TOOL_NAME: &str = "cancel_background_tool";
pub(super) const LIST_BACKGROUND_TOOLS_NAME: &str = "list_background_tools";

/// Build the schema for a collapsed MCP server control tool.
pub(super) fn build_manage_mcp_server_tool_schema(
    format: ToolSchemaFormat,
    server_id: &str,
    is_mounted: bool,
) -> Value {
    let display_name = display_name_for_server(server_id);
    let name = mount_tool_name_for_server(server_id);
    let description = format!(
        "Controls the MCP server '{display_name}' ({server_id}). Use action='mount' to load its tools for this conversation, action='unmount' to remove them again, and action='status' to inspect whether it is currently mounted. If action is omitted, it defaults to '{}' for compatibility. Mounted tools remain available across future turns and app restarts until you explicitly unmount them.",
        if is_mounted { "status" } else { "mount" }
    );
    format_tool_schema(
        format,
        &name,
        &description,
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["mount", "unmount", "status"],
                    "description": "Use 'mount' to expose this server's tools in this conversation, 'unmount' to hide them again, or 'status' to inspect the current state."
                }
            }
        }),
    )
}

pub(super) fn build_start_background_tool_schema(format: ToolSchemaFormat) -> Value {
    format_tool_schema(
        format,
        START_BACKGROUND_TOOL_NAME,
        "Starts one currently available native or MCP tool in the background and returns immediately with a jobId. Use this when a tool may take a long time and you can make progress elsewhere before waiting for the result. Waiting is a separate awaiter step: later call wait_background_tool only when the result is on your critical path, or call list_background_tools to inspect progress. Do not use this for MCP server mount/unmount control tools.",
        json!({
            "type": "object",
            "properties": {
                "toolName": {
                    "type": "string",
                    "description": "The exact active tool name to run in the background, for example 'calculator' or 'mcp__server__tool'."
                },
                "arguments": {
                    "type": "object",
                    "description": "Arguments to pass to the target tool. Use an empty object when the target tool takes no arguments."
                }
            },
            "required": ["toolName", "arguments"]
        }),
    )
}

pub(super) fn build_wait_background_tool_schema(format: ToolSchemaFormat) -> Value {
    format_tool_schema(
        format,
        WAIT_BACKGROUND_TOOL_NAME,
        "Awaiter checkpoint for a background tool job. Waits briefly for completion, or returns the current in-progress status and next actions when the timeout elapses. Use this only when the result is on your critical path; otherwise continue useful work and check later. Prefer short timeoutMs values for polling. Long waits must be an explicit choice.",
        json!({
            "type": "object",
            "properties": {
                "jobId": {
                    "type": "string",
                    "description": "The jobId returned by start_background_tool."
                },
                "timeoutMs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": background_jobs::MAX_WAIT_TIMEOUT_MS,
                    "default": background_jobs::DEFAULT_WAIT_TIMEOUT_MS,
                    "description": "Optional awaiter timeout in milliseconds. Defaults to a short 5000ms checkpoint and is capped at 120000. Use a small value when deciding whether to continue other work or wait again."
                }
            },
            "required": ["jobId"]
        }),
    )
}

pub(super) fn build_cancel_background_tool_schema(format: ToolSchemaFormat) -> Value {
    format_tool_schema(
        format,
        CANCEL_BACKGROUND_TOOL_NAME,
        "Cancels a background tool job if it is still running. Use this when the result is no longer needed or the job is taking the wrong path.",
        json!({
            "type": "object",
            "properties": {
                "jobId": {
                    "type": "string",
                    "description": "The jobId returned by start_background_tool."
                }
            },
            "required": ["jobId"]
        }),
    )
}

pub(super) fn build_list_background_tools_schema(format: ToolSchemaFormat) -> Value {
    format_tool_schema(
        format,
        LIST_BACKGROUND_TOOLS_NAME,
        "Lists background tool jobs and their current lifecycle state. Use this to inspect jobs before deciding whether to wait.",
        json!({
            "type": "object",
            "properties": {}
        }),
    )
}

/// Normalize MCP tool descriptors from varied server implementations.
pub(super) fn normalize_mcp_tool_definitions(raw_tools: Vec<Value>) -> Vec<MountedToolDefinition> {
    let mut normalized = Vec::new();
    for raw_tool in raw_tools {
        let tool_name = raw_tool
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        if tool_name.is_empty() {
            continue;
        }

        let description = raw_tool
            .get("description")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let display_name = raw_tool
            .get("title")
            .or_else(|| raw_tool.get("displayName"))
            .or_else(|| raw_tool.get("display_name"))
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| humanize_tool_name(&tool_name));
        let icon = raw_tool
            .get("icon")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .or(Some("LayoutGrid".to_string()));
        let input_schema = raw_tool
            .get("inputSchema")
            .or_else(|| raw_tool.get("input_schema"))
            .cloned()
            .unwrap_or(json!({
                "type": "object",
                "properties": {}
            }));

        normalized.push(MountedToolDefinition {
            tool_name,
            display_name,
            description,
            icon,
            input_schema,
        });
    }
    normalized
}
