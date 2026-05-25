use rmcp::{
    model::CallToolRequestParams, service::RoleClient, transport::TokioChildProcess, ServiceExt,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::process::Command;
use tokio::sync::Mutex;

pub struct McpManager {
    services: Arc<Mutex<BTreeMap<String, rmcp::service::RunningService<RoleClient, ()>>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            services: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    pub async fn get_peer(
        &self,
        server_id: &str,
        config: &crate::config::McpServerConfig,
    ) -> Result<rmcp::Peer<RoleClient>, String> {
        let mut services = self.services.lock().await;
        if let Some(service) = services.get(server_id) {
            return Ok(service.peer().clone());
        }

        if !config.enabled {
            return Err(format!("MCP server '{}' is disabled", server_id));
        }

        let mut cmd = Command::new(&config.command);
        for arg in &config.args {
            cmd.arg(arg);
        }
        for (k, v) in &config.env {
            cmd.env(k, v);
        }
        if let Some(cwd) = &config.cwd {
            cmd.current_dir(cwd);
        }

        let transport = TokioChildProcess::new(cmd)
            .map_err(|e| format!("Failed to create stdio transport: {e}"))?;

        let service = ()
            .serve(transport)
            .await
            .map_err(|e| format!("Failed to initialize MCP connection: {e}"))?;

        let peer = service.peer().clone();
        services.insert(server_id.to_string(), service);
        Ok(peer)
    }

    pub async fn list_tools(
        &self,
        server_id: &str,
        config: &crate::config::McpServerConfig,
    ) -> Result<Vec<Value>, String> {
        let peer = self.get_peer(server_id, config).await?;

        let response = peer
            .list_tools(None)
            .await
            .map_err(|e| format!("Failed to list tools from MCP server '{server_id}': {e}"))?;

        let mut tool_schemas = Vec::new();
        for tool in response.tools {
            tool_schemas.push(serde_json::to_value(&tool).unwrap_or_default());
        }

        Ok(tool_schemas)
    }

    pub async fn call_tool(
        &self,
        server_id: &str,
        config: &crate::config::McpServerConfig,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let peer = self.get_peer(server_id, config).await?;

        let args_map = match arguments {
            Value::Object(map) => Some(map),
            _ => None,
        };

        let mut param = CallToolRequestParams::new(name.to_string());
        if let Some(args) = args_map {
            param = param.with_arguments(args);
        }

        let response = peer
            .call_tool(param)
            .await
            .map_err(|e| format!("Failed to call tool '{name}' on server '{server_id}': {e}"))?;

        Ok(serde_json::to_value(response).unwrap_or_default())
    }
}
