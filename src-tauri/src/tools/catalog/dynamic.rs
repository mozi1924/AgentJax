use super::ToolCatalog;
use super::names::fallback_icon_for_dynamic_binding;
use super::snapshot::{ToolSnapshotEntry, insert_snapshot_tool};
use crate::tools::{
    ToolExecutionContext, ToolPresentation, ToolSchemaFormat, format_tool_schema,
    humanize_tool_name,
};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

impl ToolCatalog {
    pub(super) fn apply_conversation_dynamic_tools(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
        schemas: &mut Vec<Value>,
        active_tool_names: &mut HashSet<String>,
        entries: &mut HashMap<String, ToolSnapshotEntry>,
        presentations: &mut HashMap<String, ToolPresentation>,
    ) -> crate::error::AgentJaxResult<()> {
        let conversation_id = context
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(conversation_id) = conversation_id else {
            return Ok(());
        };

        let dynamic_tools =
            crate::conversation_store::load_conversation_dynamic_tools(conversation_id)?;
        for dynamic_tool in dynamic_tools {
            let crate::conversation_store::ConversationDynamicTool {
                name,
                display_name,
                description,
                icon,
                parameters,
                binding,
            } = dynamic_tool;
            let fallback_icon = fallback_icon_for_dynamic_binding(&binding, &self.native_tools);
            let entry = match binding {
                crate::conversation_store::ConversationDynamicToolBinding::Native { tool } => {
                    if !self.native_tool_enabled(&tool) {
                        log::warn!(
                            "Skipping conversation dynamic tool '{}' because native target '{}' is disabled by tool_manager",
                            name,
                            tool
                        );
                        continue;
                    }
                    let Some(native_tool) = self
                        .native_tools
                        .iter()
                        .find(|candidate| candidate.name() == tool)
                    else {
                        log::warn!(
                            "Skipping conversation dynamic tool '{}' because native target '{}' was not found",
                            name,
                            tool
                        );
                        continue;
                    };
                    ToolSnapshotEntry::Native(native_tool.clone())
                }
                crate::conversation_store::ConversationDynamicToolBinding::Mcp {
                    server_id,
                    tool,
                } => {
                    let Some(server_config) = self.mcp_config.get(&server_id) else {
                        log::warn!(
                            "Skipping conversation dynamic tool '{}' because MCP server '{}' config was not found",
                            name,
                            server_id
                        );
                        continue;
                    };
                    if !server_config.enabled {
                        log::warn!(
                            "Skipping conversation dynamic tool '{}' because MCP server '{}' is disabled",
                            name,
                            server_id
                        );
                        continue;
                    }
                    if !self.mcp_source_enabled(&server_id)
                        || !self.mcp_tool_enabled(&server_id, &tool)
                    {
                        log::warn!(
                            "Skipping conversation dynamic tool '{}' because MCP target '{}::{}' is disabled by tool_manager",
                            name,
                            server_id,
                            tool
                        );
                        continue;
                    }

                    ToolSnapshotEntry::Mcp {
                        server_id,
                        tool_name: tool,
                        server_config: self
                            .resolve_server_config_with_workspace_fallback(server_config, context),
                    }
                }
            };

            presentations.insert(
                name.clone(),
                ToolPresentation {
                    display_name: display_name.unwrap_or_else(|| humanize_tool_name(&name)),
                    description: description.clone(),
                    icon: icon.or(fallback_icon),
                },
            );
            insert_snapshot_tool(
                schemas,
                format_tool_schema(format, &name, &description, parameters),
                active_tool_names,
                entries,
                name,
                entry,
            );
        }

        Ok(())
    }
}
