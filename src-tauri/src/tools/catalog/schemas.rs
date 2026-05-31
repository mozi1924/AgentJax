use super::names::{display_name_for_server, mount_tool_name_for_server};
use super::types::MountedToolDefinition;
use crate::tools::{ToolSchemaFormat, background_jobs, format_tool_schema, humanize_tool_name};
use serde_json::{Value, json};

pub(super) const BACKGROUND_TASK_NAME: &str = "background_task";

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

/// Consolidated background task tool schema.
///
/// A single tool that handles all background lifecycle operations:
/// start, wait, cancel, and list. The `action` field selects which
/// operation to perform. This replaces the old four-tool design
/// (start_background_tool / wait_background_tool / cancel_background_tool /
/// list_background_tools).
pub(super) fn build_background_task_schema(format: ToolSchemaFormat) -> Value {
    format_tool_schema(
        format,
        BACKGROUND_TASK_NAME,
        "Manages background tool jobs — start a tool and keep working, wait/check later, cancel, or list all jobs. Use action='start' to launch a native or MCP tool in a background task and get back a jobId immediately. Later use action='wait' as an awaiter checkpoint, action='cancel' to abort a running job, or action='list' to inspect all jobs. The start action returns the jobId plus usage hints for subsequent actions.",
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["start", "wait", "cancel", "list"],
                    "description": "Which background operation to perform: 'start' to launch a tool, 'wait' to await completion, 'cancel' to abort, 'list' to enumerate all jobs."
                },
                "jobId": {
                    "type": "string",
                    "description": "Required for 'wait' and 'cancel'. The jobId returned by a previous 'start' action."
                },
                "toolName": {
                    "type": "string",
                    "description": "Required for 'start'. The exact active tool name to run in the background, e.g. 'calculator' or 'mcp__server__tool'."
                },
                "arguments": {
                    "type": "object",
                    "description": "Required for 'start'. Arguments to pass to the target tool. Use an empty object when the target tool takes no arguments."
                },
                "timeoutMs": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": background_jobs::MAX_WAIT_TIMEOUT_MS,
                    "default": background_jobs::DEFAULT_WAIT_TIMEOUT_MS,
                    "description": "Optional, for 'wait' only. Awaiter timeout in milliseconds. Defaults to 5000, capped at 120000. Use a small value when deciding whether to continue other work or wait again."
                }
            },
            "required": ["action"]
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
