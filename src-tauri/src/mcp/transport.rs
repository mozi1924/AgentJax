use crate::mcp::types::McpConnectionSpec;
use rmcp::service::RoleClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use rmcp::ServiceExt;
use std::collections::HashMap;
use tokio::process::Command;

pub async fn start_transport(
    spec: &McpConnectionSpec,
) -> Result<rmcp::service::RunningService<RoleClient, ()>, String> {
    match spec {
        McpConnectionSpec::Stdio {
            command,
            args,
            env,
            cwd,
            inherit_parent_env,
        } => {
            let mut cmd = Command::new(command);
            if !inherit_parent_env {
                cmd.env_clear();
            }
            cmd.args(args);
            cmd.envs(env);
            if let Some(working_dir) = cwd {
                cmd.current_dir(working_dir);
            }

            let transport = TokioChildProcess::new(cmd)
                .map_err(|e| format!("Failed to create stdio transport: {e}"))?;

            ().serve(transport)
                .await
                .map_err(|e| format!("Failed to initialize MCP stdio connection: {e}"))
        }
        McpConnectionSpec::StreamableHttp {
            uri,
            auth_header,
            headers,
            allow_stateless,
            channel_buffer_capacity,
            reinit_on_expired_session,
        } => {
            let mut config = StreamableHttpClientTransportConfig::with_uri(uri.clone());
            config.allow_stateless = *allow_stateless;
            config.reinit_on_expired_session = *reinit_on_expired_session;
            if let Some(capacity) = channel_buffer_capacity {
                config.channel_buffer_capacity = *capacity;
            }
            if let Some(value) = auth_header {
                config = config.auth_header(strip_bearer_prefix(value));
            }
            if !headers.is_empty() {
                config = config.custom_headers(parse_headers(headers)?);
            }

            let transport = StreamableHttpClientTransport::from_config(config);
            ().serve(transport)
                .await
                .map_err(|e| format!("Failed to initialize MCP streamable_http connection: {e}"))
        }
    }
}

fn strip_bearer_prefix(token: &str) -> String {
    token
        .trim()
        .strip_prefix("Bearer ")
        .or_else(|| token.trim().strip_prefix("bearer "))
        .unwrap_or(token.trim())
        .to_string()
}

fn parse_headers(
    headers: &std::collections::BTreeMap<String, String>,
) -> Result<HashMap<reqwest::header::HeaderName, reqwest::header::HeaderValue>, String> {
    let mut parsed = HashMap::new();
    for (name, value) in headers {
        let header_name = reqwest::header::HeaderName::from_bytes(name.as_bytes())
            .map_err(|e| format!("Invalid HTTP header name '{name}': {e}"))?;
        let header_value = reqwest::header::HeaderValue::from_str(value)
            .map_err(|e| format!("Invalid HTTP header value for '{name}': {e}"))?;
        parsed.insert(header_name, header_value);
    }

    Ok(parsed)
}
