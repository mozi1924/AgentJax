use crate::tools::{
    format_tool_schema, humanize_tool_name, EditFileTool, CalculatorTool, FileReaderTool,
    FileWriterTool, ListFilesTool, MkdirTool, SystemTimeTool, Tool, ToolExecutionContext, ToolPresentation,
    ToolSchemaFormat,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct MountedToolDefinition {
    pub tool_name: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub input_schema: Value,
}

#[derive(Debug, Clone)]
pub struct MountedToolSourceSession {
    pub source_id: String,
    pub source_type: String,
    pub tools: Vec<MountedToolDefinition>,
    pub mcp_config: Option<crate::config::McpServerConfig>,
}

pub type MountedToolSourceSessions = BTreeMap<String, MountedToolSourceSession>;

#[derive(Debug, Clone)]
pub enum ToolCatalogStateChange {
    MountToolSource(MountedToolSourceSession),
    UnmountToolSource { source_id: String, source_type: String },
}

#[derive(Debug, Clone)]
pub(crate) struct ToolCatalogExecution {
    pub output: Value,
    pub state_changes: Vec<ToolCatalogStateChange>,
}

enum ToolSnapshotEntry {
    Native(Arc<dyn Tool>),
    Mcp {
        server_id: String,
        tool_name: String,
        server_config: crate::config::McpServerConfig,
    },
    ManageMcpServer {
        server_id: String,
        server_config: crate::config::McpServerConfig,
        mounted_session: Option<MountedToolSourceSession>,
    },
}

fn insert_snapshot_tool(
    schemas: &mut Vec<Value>,
    schema: Value,
    active_tool_names: &mut HashSet<String>,
    entries: &mut HashMap<String, ToolSnapshotEntry>,
    tool_name: String,
    entry: ToolSnapshotEntry,
) {
    active_tool_names.insert(tool_name.clone());
    entries.insert(tool_name.clone(), entry);

    if let Some(existing_idx) = schemas.iter().position(|existing| {
        existing.get("name").and_then(Value::as_str) == Some(tool_name.as_str())
            || existing
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                == Some(tool_name.as_str())
    }) {
        schemas[existing_idx] = schema;
    } else {
        schemas.push(schema);
    }
}

fn mount_tool_name_for_server(server_id: &str) -> String {
    format!("mcp_server__{server_id}")
}

fn prefixed_mcp_tool_name(server_id: &str, tool_name: &str) -> String {
    format!("mcp__{server_id}__{tool_name}")
}

fn display_name_for_server(server_id: &str) -> String {
    humanize_tool_name(server_id)
}

fn presentation_for_manage_mcp_server(server_id: &str) -> ToolPresentation {
    let display_name = display_name_for_server(server_id);
    ToolPresentation::new(
        format!("Manage {}", display_name),
        format!("Controls the MCP server '{}'.", display_name),
        Some("LayoutGrid"),
    )
}

fn fallback_icon_for_dynamic_binding(
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

fn build_manage_mcp_server_tool_schema(
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

fn normalize_mcp_tool_definitions(raw_tools: Vec<Value>) -> Vec<MountedToolDefinition> {
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

/// Turn-scoped tool snapshot.
///
/// The model-visible tool list and local execution dispatch both read from the
/// same frozen snapshot so a turn cannot drift if MCP tools are reconfigured or
/// refreshed midway through a tool loop.
pub struct ToolCatalogSnapshot {
    schemas: Vec<Value>,
    active_tool_names: HashSet<String>,
    entries: HashMap<String, ToolSnapshotEntry>,
    presentations: HashMap<String, ToolPresentation>,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_runtime: crate::config::McpRuntimeConfig,
}

impl ToolCatalogSnapshot {
    pub fn schemas(&self) -> &[Value] {
        &self.schemas
    }

    pub fn active_tool_names(&self) -> &HashSet<String> {
        &self.active_tool_names
    }

    pub fn presentation_for(&self, tool_name: &str) -> Option<&ToolPresentation> {
        self.presentations.get(tool_name)
    }

    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        Ok(self
            .execute_with_effects(tool_name, arguments, context)
            .await?
            .output)
    }

    pub(crate) async fn execute_with_effects(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<ToolCatalogExecution, String> {
        let entry = self
            .entries
            .get(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found in turn snapshot", tool_name))?;

        match entry {
            ToolSnapshotEntry::Native(tool) => Ok(ToolCatalogExecution {
                output: tool.execute(arguments, context)?,
                state_changes: Vec::new(),
            }),
            ToolSnapshotEntry::Mcp {
                server_id,
                tool_name,
                server_config,
            } => Ok(ToolCatalogExecution {
                output: self
                    .mcp_manager
                    .call_tool(
                        server_id,
                        server_config,
                        &self.mcp_runtime,
                        tool_name,
                        arguments.clone(),
                    )
                    .await?,
                state_changes: Vec::new(),
            }),
            ToolSnapshotEntry::ManageMcpServer {
                server_id,
                server_config,
                mounted_session,
            } => {
                let requested_action = arguments
                    .get("action")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .map(|value| value.to_ascii_lowercase());
                let action = requested_action
                    .as_deref()
                    .unwrap_or(if mounted_session.is_some() {
                        "status"
                    } else {
                        "mount"
                    });
                let control_tool = mount_tool_name_for_server(server_id);

                match action {
                    "status" => {
                        let mounted_tools = mounted_session
                            .as_ref()
                            .map(|session| {
                                session
                                    .tools
                                    .iter()
                                    .map(|tool| prefixed_mcp_tool_name(server_id, &tool.tool_name))
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        Ok(ToolCatalogExecution {
                            output: json!({
                                "serverId": server_id,
                                "controlTool": control_tool,
                                "mounted": mounted_session.is_some(),
                                "mountedTools": mounted_tools,
                                "status": if mounted_session.is_some() { "mounted" } else { "unmounted" },
                            }),
                            state_changes: Vec::new(),
                        })
                    }
                    "mount" => {
                        if let Some(session) = mounted_session {
                            let mounted_tools = session
                                .tools
                                .iter()
                                .map(|tool| prefixed_mcp_tool_name(server_id, &tool.tool_name))
                                .collect::<Vec<_>>();
                            return Ok(ToolCatalogExecution {
                                output: json!({
                                    "serverId": server_id,
                                    "controlTool": control_tool,
                                    "mounted": true,
                                    "mountedToolCount": mounted_tools.len(),
                                    "mountedTools": mounted_tools,
                                    "status": "already_mounted",
                                }),
                                state_changes: Vec::new(),
                            });
                        }

                        let raw_tools = self
                            .mcp_manager
                            .list_tools(server_id, server_config, &self.mcp_runtime)
                            .await?;
                        let mounted_tools = normalize_mcp_tool_definitions(raw_tools);
                        if mounted_tools.is_empty() {
                            return Err(format!(
                                "MCP server '{}' did not expose any tools to mount",
                                server_id
                            ));
                        }

                        let mounted_tool_names = mounted_tools
                            .iter()
                            .map(|tool| prefixed_mcp_tool_name(server_id, &tool.tool_name))
                            .collect::<Vec<_>>();
                        Ok(ToolCatalogExecution {
                            output: json!({
                                "serverId": server_id,
                                "controlTool": control_tool,
                                "mounted": true,
                                "mountedToolCount": mounted_tool_names.len(),
                                "mountedTools": mounted_tool_names,
                                "status": "mounted",
                                "scope": "conversation",
                                "usage": {
                                    "mount": { "action": "mount" },
                                    "unmount": { "action": "unmount" },
                                    "status": { "action": "status" }
                                }
                            }),
                            state_changes: vec![ToolCatalogStateChange::MountToolSource(
                                MountedToolSourceSession {
                                    source_id: server_id.clone(),
                                    source_type: "mcp".to_string(),
                                    tools: mounted_tools,
                                    mcp_config: Some(server_config.clone()),
                                },
                            )],
                        })
                    }
                    "unmount" => Ok(ToolCatalogExecution {
                        output: json!({
                            "serverId": server_id,
                            "controlTool": control_tool,
                            "mounted": false,
                            "status": if mounted_session.is_some() { "unmounted" } else { "already_unmounted" },
                        }),
                        state_changes: if mounted_session.is_some() {
                            vec![ToolCatalogStateChange::UnmountToolSource {
                                source_id: server_id.clone(),
                                source_type: "mcp".to_string(),
                            }]
                        } else {
                            Vec::new()
                        },
                    }),
                    _ => Err(format!(
                        "Unsupported action '{}' for MCP server control tool '{}'. Use one of: mount, unmount, status.",
                        action, control_tool
                    )),
                }
            }
        }
    }
}

pub struct ToolCatalog {
    native_tools: Vec<Arc<dyn Tool>>,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_runtime: crate::config::McpRuntimeConfig,
    mcp_config: BTreeMap<String, crate::config::McpServerConfig>,
}

impl ToolCatalog {
    pub fn new(
        mcp_manager: Arc<crate::mcp::McpManager>,
        config: &crate::config::AppConfig,
    ) -> Self {
        Self {
            native_tools: vec![
                Arc::new(CalculatorTool),
                Arc::new(SystemTimeTool),
                Arc::new(FileReaderTool),
                Arc::new(FileWriterTool),
                Arc::new(ListFilesTool),
                Arc::new(MkdirTool),
                Arc::new(EditFileTool),
            ],
            mcp_manager,
            mcp_runtime: config.mcp_runtime.clone(),
            mcp_config: config.mcp_servers.clone(),
        }
    }

    pub async fn snapshot(&self, context: &ToolExecutionContext) -> ToolCatalogSnapshot {
        let mounted_servers = self.load_persisted_mounted_servers(context);
        self.snapshot_with_format_and_mounted_servers(
            ToolSchemaFormat::Responses,
            context,
            &mounted_servers,
        )
        .await
    }

    pub async fn snapshot_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> ToolCatalogSnapshot {
        let mounted_servers = self.load_persisted_mounted_servers(context);
        self.snapshot_with_format_and_mounted_servers(format, context, &mounted_servers)
            .await
    }

    pub(crate) async fn snapshot_with_format_and_mounted_servers(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
        mounted_servers: &MountedToolSourceSessions,
    ) -> ToolCatalogSnapshot {
        let mut schemas = Vec::new();
        let mut active_tool_names = HashSet::new();
        let mut entries = HashMap::new();
        let mut presentations = HashMap::new();

        for tool in &self.native_tools {
            let schema = tool.to_schema_with_format(format);
            let tool_name = tool.name().to_string();
            presentations.insert(tool_name.clone(), tool.presentation());
            insert_snapshot_tool(
                &mut schemas,
                schema,
                &mut active_tool_names,
                &mut entries,
                tool_name,
                ToolSnapshotEntry::Native(tool.clone()),
            );
        }

        for (server_id, server_config) in &self.mcp_config {
            if !server_config.enabled {
                continue;
            }

            let resolved_server_config =
                self.resolve_server_config_with_workspace_fallback(server_config, context);

            if server_config.unfolded {
                match self
                    .mcp_manager
                    .list_tools(server_id, &resolved_server_config, &self.mcp_runtime)
                    .await
                {
                    Ok(raw_tools) => {
                        let mounted_tools = normalize_mcp_tool_definitions(raw_tools);
                        for tool in mounted_tools {
                            let prefixed_name = prefixed_mcp_tool_name(server_id, &tool.tool_name);
                            presentations.insert(
                                prefixed_name.clone(),
                                ToolPresentation {
                                    display_name: tool.display_name.clone(),
                                    description: tool.description.clone(),
                                    icon: tool.icon.clone(),
                                },
                            );
                            insert_snapshot_tool(
                                &mut schemas,
                                format_tool_schema(
                                    format,
                                    &prefixed_name,
                                    &tool.description,
                                    tool.input_schema.clone(),
                                ),
                                &mut active_tool_names,
                                &mut entries,
                                prefixed_name,
                                ToolSnapshotEntry::Mcp {
                                    server_id: server_id.clone(),
                                    tool_name: tool.tool_name.clone(),
                                    server_config: resolved_server_config.clone(),
                                },
                            );
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "Failed to list tools for unfolded MCP server '{}': {}",
                            server_id,
                            err
                        );
                    }
                }
            } else {
                let mounted_session = mounted_servers.get(server_id).cloned();
                let control_tool_name = mount_tool_name_for_server(server_id);
                presentations.insert(
                    control_tool_name.clone(),
                    presentation_for_manage_mcp_server(server_id),
                );
                insert_snapshot_tool(
                    &mut schemas,
                    build_manage_mcp_server_tool_schema(format, server_id, mounted_session.is_some()),
                    &mut active_tool_names,
                    &mut entries,
                    control_tool_name,
                    ToolSnapshotEntry::ManageMcpServer {
                        server_id: server_id.clone(),
                        server_config: resolved_server_config.clone(),
                        mounted_session: mounted_session.clone(),
                    },
                );

                if let Some(mounted) = mounted_servers.get(server_id) {
                    for tool in &mounted.tools {
                        let prefixed_name = prefixed_mcp_tool_name(server_id, &tool.tool_name);
                        presentations.insert(
                            prefixed_name.clone(),
                            ToolPresentation {
                                display_name: tool.display_name.clone(),
                                description: tool.description.clone(),
                                icon: tool.icon.clone(),
                            },
                        );
                        insert_snapshot_tool(
                            &mut schemas,
                            format_tool_schema(
                                format,
                                &prefixed_name,
                                &tool.description,
                                tool.input_schema.clone(),
                            ),
                            &mut active_tool_names,
                            &mut entries,
                            prefixed_name,
                            ToolSnapshotEntry::Mcp {
                                server_id: mounted.source_id.clone(),
                                tool_name: tool.tool_name.clone(),
                                server_config: mounted
                                    .mcp_config
                                    .as_ref()
                                    .cloned()
                                    .unwrap_or_else(|| resolved_server_config.clone()),
                            },
                        );
                    }
                }
            }
        }

        if let Err(err) = self.apply_conversation_dynamic_tools(
            format,
            context,
            &mut schemas,
            &mut active_tool_names,
            &mut entries,
            &mut presentations,
        ) {
            log::warn!(
                "Failed to apply conversation dynamic tools for {:?}: {}",
                context.conversation_id,
                err
            );
        }

        ToolCatalogSnapshot {
            schemas,
            active_tool_names,
            entries,
            presentations,
            mcp_manager: self.mcp_manager.clone(),
            mcp_runtime: self.mcp_runtime.clone(),
        }
    }

    pub async fn list_schemas(&self, context: &ToolExecutionContext) -> Vec<Value> {
        self.snapshot(context).await.schemas
    }

    pub async fn list_schemas_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> Vec<Value> {
        self.snapshot_with_format(format, context).await.schemas
    }

    pub async fn execute(
        &self,
        prefixed_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        self.snapshot(context)
            .await
            .execute(prefixed_name, arguments, context)
            .await
    }

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

        let stored_sources =
            match crate::conversation_store::load_conversation_mounted_tool_sources(conversation_id)
            {
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
                        mcp_config: Some(self.resolve_server_config_with_workspace_fallback(server_config, context)),
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
                        .map(|tool| {
                            crate::conversation_store::ConversationMountedToolDefinition {
                                tool_name: tool.tool_name.clone(),
                                display_name: tool.display_name.clone(),
                                description: tool.description.clone(),
                                icon: tool.icon.clone(),
                                input_schema: tool.input_schema.clone(),
                            }
                        })
                        .collect(),
                },
            )
            .collect::<Vec<_>>();
        crate::conversation_store::update_conversation_mounted_tool_sources(
            conversation_id,
            persisted_sources,
        )
    }

    fn resolve_server_config_with_workspace_fallback(
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

    fn apply_conversation_dynamic_tools(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
        schemas: &mut Vec<Value>,
        active_tool_names: &mut HashSet<String>,
        entries: &mut HashMap<String, ToolSnapshotEntry>,
        presentations: &mut HashMap<String, ToolPresentation>,
    ) -> Result<(), String> {
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
