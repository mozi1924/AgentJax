mod transport;
mod types;

use rmcp::{model::CallToolRequestParams, service::RoleClient};
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use types::resolve_server_runtime;

struct ManagedService {
    fingerprint: String,
    service: rmcp::service::RunningService<RoleClient, ()>,
}

pub struct McpManager {
    services: Arc<Mutex<BTreeMap<String, ManagedService>>>,
    server_locks: Arc<Mutex<BTreeMap<String, Arc<Mutex<()>>>>>,
}

impl McpManager {
    pub fn new() -> Self {
        Self {
            services: Arc::new(Mutex::new(BTreeMap::new())),
            server_locks: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }

    async fn server_lock(&self, server_id: &str) -> Arc<Mutex<()>> {
        let mut locks = self.server_locks.lock().await;
        locks
            .entry(server_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    pub async fn get_peer(
        &self,
        server_id: &str,
        config: &crate::config::McpServerConfig,
        runtime_config: &crate::config::McpRuntimeConfig,
    ) -> Result<rmcp::Peer<RoleClient>, String> {
        let resolved = resolve_server_runtime(server_id, config, runtime_config)?;
        if !resolved.enabled {
            self.shutdown_service(server_id).await;
            return Err(format!("MCP server '{}' is disabled", server_id));
        }

        let server_lock = self.server_lock(server_id).await;
        let _guard = server_lock.lock().await;

        {
            let services = self.services.lock().await;
            if let Some(entry) = services.get(server_id) {
                if entry.fingerprint == resolved.fingerprint {
                    return Ok(entry.service.peer().clone());
                }
            }
        }

        self.shutdown_service(server_id).await;

        let service = tokio::time::timeout(
            Duration::from_millis(runtime_config.startup_timeout_ms),
            transport::start_transport(&resolved.connection),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out while starting MCP server '{}' after {}ms",
                server_id, runtime_config.startup_timeout_ms
            )
        })??;
        let peer = service.peer().clone();
        let mut services = self.services.lock().await;
        services.insert(
            server_id.to_string(),
            ManagedService {
                fingerprint: resolved.fingerprint,
                service,
            },
        );
        Ok(peer)
    }

    async fn shutdown_service(&self, server_id: &str) {
        let removed = {
            let mut services = self.services.lock().await;
            services.remove(server_id)
        };

        let Some(mut entry) = removed else {
            return;
        };

        match tokio::time::timeout(Duration::from_secs(3), entry.service.close()).await {
            Ok(Ok(reason)) => {
                log::info!("Closed MCP server '{}' with reason {:?}", server_id, reason);
            }
            Ok(Err(err)) => {
                log::warn!("Failed to close MCP server '{}': {}", server_id, err);
            }
            Err(_) => {
                log::warn!(
                    "Timed out while closing MCP server '{}'; dropping transport",
                    server_id
                );
            }
        }
    }

    pub async fn list_tools(
        &self,
        server_id: &str,
        config: &crate::config::McpServerConfig,
        runtime_config: &crate::config::McpRuntimeConfig,
    ) -> Result<Vec<Value>, String> {
        let peer = self.get_peer(server_id, config, runtime_config).await?;

        let response = tokio::time::timeout(
            Duration::from_millis(runtime_config.tool_timeout_ms),
            peer.list_tools(None),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out while listing tools from MCP server '{}' after {}ms",
                server_id, runtime_config.tool_timeout_ms
            )
        })?
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

        let response = tokio::time::timeout(
            Duration::from_millis(runtime_config.tool_timeout_ms),
            peer.call_tool(param),
        )
        .await
        .map_err(|_| {
            format!(
                "Timed out while calling tool '{}' on MCP server '{}' after {}ms",
                name, server_id, runtime_config.tool_timeout_ms
            )
        })?
        .map_err(|e| format!("Failed to call tool '{name}' on server '{server_id}': {e}"))?;

        Ok(serde_json::to_value(response).unwrap_or_default())
    }
}
