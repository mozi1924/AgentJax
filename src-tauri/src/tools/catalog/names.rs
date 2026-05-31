use crate::tools::{Tool, ToolPresentation, humanize_tool_name};
use std::sync::Arc;

/// Build the stable control-tool name for an MCP server.
pub(super) fn mount_tool_name_for_server(server_id: &str) -> String {
    format!("mcp_server__{server_id}")
}

/// Build the stable model-visible name for a mounted MCP tool.
pub(super) fn prefixed_mcp_tool_name(server_id: &str, tool_name: &str) -> String {
    format!("mcp__{server_id}__{tool_name}")
}

pub(super) fn display_name_for_server(server_id: &str) -> String {
    humanize_tool_name(server_id)
}

pub(super) fn presentation_for_manage_mcp_server(server_id: &str) -> ToolPresentation {
    let display_name = display_name_for_server(server_id);
    ToolPresentation::new(
        format!("Manage {}", display_name),
        format!("Controls the MCP server '{}'.", display_name),
        Some("LayoutGrid"),
    )
}

/// Infer a UI icon for a conversation-defined alias from the bound tool type.
pub(super) fn fallback_icon_for_dynamic_binding(
    binding: &crate::conversation_store::ConversationDynamicToolBinding,
    native_tools: &[Arc<dyn Tool>],
) -> Option<String> {
    match binding {
        crate::conversation_store::ConversationDynamicToolBinding::Native { tool } => native_tools
            .iter()
            .find(|candidate| candidate.name() == tool)
            .and_then(|candidate| candidate.icon().map(str::to_string)),
        crate::conversation_store::ConversationDynamicToolBinding::Mcp { .. } => {
            Some("LayoutGrid".to_string())
        }
    }
}
