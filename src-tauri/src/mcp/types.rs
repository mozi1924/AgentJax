use crate::agentjax_err;
use crate::config::{McpRuntimeConfig, McpServerConfig, McpTransportKind};
use crate::error::{AgentJaxError, AgentJaxResult};
use serde::Serialize;
use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub struct McpResolvedServerRuntime {
    pub enabled: bool,
    pub fingerprint: String,
    pub connection: McpConnectionSpec,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "transport")]
pub enum McpConnectionSpec {
    Stdio {
        command: String,
        args: Vec<String>,
        env: BTreeMap<String, String>,
        cwd: Option<String>,
        inherit_parent_env: bool,
    },
    StreamableHttp {
        uri: String,
        auth_header: Option<String>,
        headers: BTreeMap<String, String>,
        allow_stateless: bool,
        channel_buffer_capacity: Option<usize>,
        reinit_on_expired_session: bool,
    },
}

pub fn resolve_server_runtime(
    server_id: &str,
    server_config: &McpServerConfig,
    runtime_config: &McpRuntimeConfig,
) -> AgentJaxResult<McpResolvedServerRuntime> {
    let connection = match server_config.transport {
        McpTransportKind::Stdio => {
            if server_config.command.is_empty() {
                return Err(agentjax_err!(
                    format!(
                        "MCP server '{}' requires `command` for stdio transport",
                        server_id
                    ),
                    Config
                ));
            }

            let mut env = BTreeMap::new();
            if server_config.use_global_stdio_env {
                env.extend(runtime_config.stdio.env.clone());
            }
            env.extend(server_config.env.clone());

            McpConnectionSpec::Stdio {
                command: server_config.command.clone(),
                args: server_config.args.clone(),
                env,
                cwd: server_config.cwd.clone(),
                inherit_parent_env: server_config
                    .inherit_parent_env
                    .unwrap_or(runtime_config.stdio.inherit_parent_env),
            }
        }
        McpTransportKind::StreamableHttp => {
            let uri = server_config.uri.clone().ok_or_else(|| {
                agentjax_err!(
                    format!(
                        "MCP server '{}' requires `uri` for streamable_http transport",
                        server_id
                    ),
                    Config
                )
            })?;

            McpConnectionSpec::StreamableHttp {
                uri,
                auth_header: server_config.auth_header.clone(),
                headers: server_config.headers.clone(),
                allow_stateless: server_config.allow_stateless,
                channel_buffer_capacity: server_config.channel_buffer_capacity,
                reinit_on_expired_session: server_config.reinit_on_expired_session,
            }
        }
    };

    let fingerprint = serde_json::to_string(&connection).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to serialize MCP server '{server_id}' config: {e}"
        ))
        .with_error_source(&e)
    })?;

    Ok(McpResolvedServerRuntime {
        enabled: server_config.enabled,
        fingerprint,
        connection,
    })
}
