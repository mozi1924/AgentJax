use crate::tools::{
    format_tool_schema, CalculatorTool, FileReaderTool, FileWriterTool, SystemTimeTool, Tool,
    ToolExecutionContext, ToolSchemaFormat,
};
use serde_json::{json, Value};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;

enum ToolSnapshotEntry {
    Native(Arc<dyn Tool>),
    Mcp {
        server_id: String,
        tool_name: String,
        server_config: crate::config::McpServerConfig,
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

/// Turn-scoped tool snapshot.
///
/// The model-visible tool list and local execution dispatch both read from the
/// same frozen snapshot so a turn cannot drift if MCP tools are reconfigured or
/// refreshed midway through a tool loop.
pub struct ToolCatalogSnapshot {
    schemas: Vec<Value>,
    active_tool_names: HashSet<String>,
    entries: HashMap<String, ToolSnapshotEntry>,
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

    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        let entry = self
            .entries
            .get(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found in turn snapshot", tool_name))?;

        match entry {
            ToolSnapshotEntry::Native(tool) => tool.execute(arguments, context),
            ToolSnapshotEntry::Mcp {
                server_id,
                tool_name,
                server_config,
            } => {
                self.mcp_manager
                    .call_tool(
                        server_id,
                        server_config,
                        &self.mcp_runtime,
                        tool_name,
                        arguments.clone(),
                    )
                    .await
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
            ],
            mcp_manager,
            mcp_runtime: config.mcp_runtime.clone(),
            mcp_config: config.mcp_servers.clone(),
        }
    }

    pub async fn snapshot(&self, context: &ToolExecutionContext) -> ToolCatalogSnapshot {
        self.snapshot_with_format(ToolSchemaFormat::Responses, context)
            .await
    }

    pub async fn snapshot_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> ToolCatalogSnapshot {
        let mut schemas = Vec::new();
        let mut active_tool_names = HashSet::new();
        let mut entries = HashMap::new();

        for tool in &self.native_tools {
            let schema = tool.to_schema_with_format(format);
            insert_snapshot_tool(
                &mut schemas,
                schema,
                &mut active_tool_names,
                &mut entries,
                tool.name().to_string(),
                ToolSnapshotEntry::Native(tool.clone()),
            );
        }

        for (server_id, server_config) in &self.mcp_config {
            if !server_config.enabled {
                continue;
            }

            let resolved_server_config =
                self.resolve_server_config_with_workspace_fallback(server_config, context);
            match self
                .mcp_manager
                .list_tools(server_id, &resolved_server_config, &self.mcp_runtime)
                .await
            {
                Ok(mcp_tools) => {
                    for raw_tool in mcp_tools {
                        let raw_name = raw_tool
                            .get("name")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .trim()
                            .to_string();
                        if raw_name.is_empty() {
                            continue;
                        }

                        let prefixed_name = format!("mcp__{}__{}", server_id, raw_name);
                        let description = raw_tool
                            .get("description")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string();
                        let input_schema = raw_tool
                            .get("inputSchema")
                            .or_else(|| raw_tool.get("input_schema"))
                            .cloned()
                            .unwrap_or(json!({
                                "type": "object",
                                "properties": {}
                            }));

                        insert_snapshot_tool(
                            &mut schemas,
                            format_tool_schema(format, &prefixed_name, &description, input_schema),
                            &mut active_tool_names,
                            &mut entries,
                            prefixed_name,
                            ToolSnapshotEntry::Mcp {
                                server_id: server_id.clone(),
                                tool_name: raw_name,
                                server_config: resolved_server_config.clone(),
                            },
                        );
                    }
                }
                Err(err) => {
                    log::warn!(
                        "Failed to list tools from MCP server '{}': {}",
                        server_id,
                        err
                    );
                }
            }
        }

        if let Err(err) = self.apply_conversation_dynamic_tools(
            format,
            context,
            &mut schemas,
            &mut active_tool_names,
            &mut entries,
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
                description,
                parameters,
                binding,
            } = dynamic_tool;
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
