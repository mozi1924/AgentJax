use crate::tools::{
    format_tool_schema, CalculatorTool, FileReaderTool, FileWriterTool, SystemTimeTool, Tool,
    ToolExecutionContext, ToolSchemaFormat,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::sync::Arc;

pub struct ToolCatalog {
    native_tools: Vec<Box<dyn Tool>>,
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
                Box::new(CalculatorTool),
                Box::new(SystemTimeTool),
                Box::new(FileReaderTool),
                Box::new(FileWriterTool),
            ],
            mcp_manager,
            mcp_runtime: config.mcp_runtime.clone(),
            mcp_config: config.mcp_servers.clone(),
        }
    }

    pub async fn list_schemas(&self, context: &ToolExecutionContext) -> Vec<Value> {
        self.list_schemas_with_format(ToolSchemaFormat::Responses, context)
            .await
    }

    pub async fn list_schemas_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> Vec<Value> {
        let mut schemas = Vec::new();

        for tool in &self.native_tools {
            schemas.push(tool.to_schema_with_format(format));
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

                        schemas.push(format_tool_schema(
                            format,
                            &prefixed_name,
                            &description,
                            input_schema,
                        ));
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

        schemas
    }

    pub async fn execute(
        &self,
        prefixed_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        if prefixed_name.starts_with("mcp__") {
            let parts: Vec<&str> = prefixed_name.split("__").collect();
            if parts.len() >= 3 && parts[0] == "mcp" {
                let server_id = parts[1];
                let tool_name = parts[2..].join("__");

                let server_config = self
                    .mcp_config
                    .get(server_id)
                    .ok_or_else(|| format!("MCP server '{}' config not found", server_id))?;
                let resolved_server_config =
                    self.resolve_server_config_with_workspace_fallback(server_config, context);

                return self
                    .mcp_manager
                    .call_tool(
                        server_id,
                        &resolved_server_config,
                        &self.mcp_runtime,
                        &tool_name,
                        arguments.clone(),
                    )
                    .await;
            }
        }

        let tool = self
            .native_tools
            .iter()
            .find(|tool| tool.name() == prefixed_name)
            .ok_or_else(|| format!("Tool '{}' not found in catalog", prefixed_name))?;

        tool.execute(arguments, context)
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
}
