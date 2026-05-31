use super::ToolCatalog;
use super::types::{MountedToolDefinition, MountedToolSourceSession, MountedToolSourceSessions};
use crate::tools::{ToolExecutionContext, humanize_tool_name};

impl ToolCatalog {
    /// Rebuild the mounted MCP server set from conversation metadata so the
    /// agent can resume previously mounted tool surfaces after later turns or
    /// an application restart.
    pub fn load_persisted_mounted_servers(
        &self,
        context: &ToolExecutionContext,
    ) -> MountedToolSourceSessions {
        let conversation_id = context
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(conversation_id) = conversation_id else {
            return MountedToolSourceSessions::new();
        };

        let stored_sources = match crate::conversation_store::load_conversation_mounted_tool_sources(
            conversation_id,
        ) {
            Ok(sources) => sources,
            Err(err) => {
                log::warn!(
                    "Failed to load mounted tool sources for conversation '{}': {}",
                    conversation_id,
                    err
                );
                return MountedToolSourceSessions::new();
            }
        };

        let mut mounted_servers = MountedToolSourceSessions::new();
        for stored_source in stored_sources {
            if stored_source.source_type == "mcp" {
                let Some(server_config) = self.mcp_config.get(&stored_source.source_id) else {
                    log::warn!(
                        "Skipping persisted mounted MCP server '{}' because its config was not found",
                        stored_source.source_id
                    );
                    continue;
                };
                if !server_config.enabled {
                    log::warn!(
                        "Skipping persisted mounted MCP server '{}' because it is disabled",
                        stored_source.source_id
                    );
                    continue;
                }

                mounted_servers.insert(
                    stored_source.source_id.clone(),
                    MountedToolSourceSession {
                        source_id: stored_source.source_id,
                        source_type: "mcp".to_string(),
                        tools: stored_source
                            .tools
                            .into_iter()
                            .map(|tool| {
                                let fallback_name = humanize_tool_name(&tool.tool_name);
                                MountedToolDefinition {
                                    tool_name: tool.tool_name,
                                    display_name: if tool.display_name.trim().is_empty() {
                                        fallback_name
                                    } else {
                                        tool.display_name
                                    },
                                    description: tool.description,
                                    icon: tool.icon.or(Some("LayoutGrid".to_string())),
                                    input_schema: tool.input_schema,
                                }
                            })
                            .collect(),
                        mcp_config: Some(
                            self.resolve_server_config_with_workspace_fallback(
                                server_config,
                                context,
                            ),
                        ),
                    },
                );
            }
        }

        mounted_servers
    }

    pub fn persist_mounted_servers(
        &self,
        conversation_id: &str,
        mounted_servers: &MountedToolSourceSessions,
    ) -> Result<(), String> {
        let persisted_sources = mounted_servers
            .values()
            .map(
                |server| crate::conversation_store::ConversationMountedToolSource {
                    source_id: server.source_id.clone(),
                    source_type: server.source_type.clone(),
                    tools: server
                        .tools
                        .iter()
                        .map(
                            |tool| crate::conversation_store::ConversationMountedToolDefinition {
                                tool_name: tool.tool_name.clone(),
                                display_name: tool.display_name.clone(),
                                description: tool.description.clone(),
                                icon: tool.icon.clone(),
                                input_schema: tool.input_schema.clone(),
                            },
                        )
                        .collect(),
                },
            )
            .collect::<Vec<_>>();
        crate::conversation_store::update_conversation_mounted_tool_sources(
            conversation_id,
            persisted_sources,
        )
    }

    pub(super) fn resolve_server_config_with_workspace_fallback(
        &self,
        server_config: &crate::config::McpServerConfig,
        context: &ToolExecutionContext,
    ) -> crate::config::McpServerConfig {
        if server_config.cwd.is_some() {
            return server_config.clone();
        }

        let conversation_id = context
            .conversation_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty());
        let Some(conversation_id) = conversation_id else {
            return server_config.clone();
        };

        let workspace =
            match crate::conversation_store::conversation_workspace_path(conversation_id) {
                Ok(path) => path,
                Err(err) => {
                    log::warn!(
                        "Failed to resolve workspace for conversation '{}' as MCP cwd fallback: {}",
                        conversation_id,
                        err
                    );
                    return server_config.clone();
                }
            };

        let mut next = server_config.clone();
        next.cwd = Some(workspace.to_string_lossy().to_string());
        next
    }
}
