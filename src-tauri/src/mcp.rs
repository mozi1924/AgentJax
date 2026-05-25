mod transport;
mod types;

use rmcp::{model::CallToolRequestParams, service::RoleClient};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tokio::sync::Mutex;
use types::resolve_server_runtime;

struct ManagedService {
    fingerprint: String,
    service: rmcp::service::RunningService<RoleClient, ()>,
}

pub struct McpManager {
    services: Arc<Mutex<BTreeMap<String, ManagedService>>>,
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
        runtime_config: &crate::config::McpRuntimeConfig,
    ) -> Result<rmcp::Peer<RoleClient>, String> {
        let resolved = resolve_server_runtime(server_id, config, runtime_config)?;
        if !resolved.enabled {
            return Err(format!("MCP server '{}' is disabled", server_id));
        }

        let mut services = self.services.lock().await;
        if let Some(entry) = services.get(server_id) {
            if entry.fingerprint == resolved.fingerprint {
                return Ok(entry.service.peer().clone());
            }

            services.remove(server_id);
        }

        let service = transport::start_transport(&resolved.connection).await?;
        let peer = service.peer().clone();
        services.insert(
            server_id.to_string(),
            ManagedService {
                fingerprint: resolved.fingerprint,
                service,
            },
        );
        Ok(peer)
    }

    pub async fn list_tools(
        &self,
        server_id: &str,
        config: &crate::config::McpServerConfig,
        runtime_config: &crate::config::McpRuntimeConfig,
    ) -> Result<Vec<Value>, String> {
        let peer = self.get_peer(server_id, config, runtime_config).await?;

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
        runtime_config: &crate::config::McpRuntimeConfig,
        name: &str,
        arguments: Value,
    ) -> Result<Value, String> {
        let peer = self.get_peer(server_id, config, runtime_config).await?;

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
