use crate::error::{AgentJaxError, AgentJaxResult};
use crate::mcp::types::McpConnectionSpec;
use rmcp::ServiceExt;
use rmcp::service::RoleClient;
use rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig;
use rmcp::transport::{StreamableHttpClientTransport, TokioChildProcess};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::path::Path;
use tokio::process::Command;

pub async fn start_transport(
    spec: &McpConnectionSpec,
) -> AgentJaxResult<rmcp::service::RunningService<RoleClient, ()>> {
    match spec {
        McpConnectionSpec::Stdio {
            command,
            args,
            env,
            cwd,
            inherit_parent_env,
        } => {
            let executable = resolve_stdio_executable(command, *inherit_parent_env, env);
            let mut cmd = Command::new(&executable);
            cmd.kill_on_drop(true);
            if !inherit_parent_env {
                cmd.env_clear();
            }
            cmd.args(args);
            cmd.envs(env);
            if let Some(working_dir) = cwd {
                cmd.current_dir(working_dir);
            }

            let transport = TokioChildProcess::new(cmd).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to create stdio transport (command='{}', resolved='{}', cwd='{}'): {e}",
                    command,
                    executable,
                    cwd.as_deref().unwrap_or(""),
                ))
                .with_error_source(&e)
            })?;

            ().serve(transport).await.map_err(|e| {
                AgentJaxError::internal(format!("Failed to initialize MCP stdio connection: {e}"))
                    .with_error_source(&e)
            })
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
            ().serve(transport).await.map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to initialize MCP streamable_http connection: {e}"
                ))
                .with_error_source(&e)
            })
        }
    }
}

fn resolve_stdio_executable(
    command: &str,
    inherit_parent_env: bool,
    env: &BTreeMap<String, String>,
) -> String {
    let trimmed = command.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if trimmed.contains(std::path::MAIN_SEPARATOR) {
        return trimmed.to_string();
    }

    let path_hint = if inherit_parent_env {
        std::env::var("PATH").ok()
    } else {
        env.get("PATH")
            .cloned()
            .or_else(|| std::env::var("PATH").ok())
    };

    let Some(path_value) = path_hint else {
        return trimmed.to_string();
    };

    for base in std::env::split_paths(&path_value) {
        let candidate = base.join(trimmed);
        if candidate.is_file()
            && let Some(resolved) = path_to_string(&candidate)
        {
            return resolved;
        }
    }

    trimmed.to_string()
}

fn path_to_string(path: &Path) -> Option<String> {
    path.to_str().map(ToOwned::to_owned)
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
) -> AgentJaxResult<HashMap<reqwest::header::HeaderName, reqwest::header::HeaderValue>> {
    crate::http_util::parse_headers_map(headers)
}

#[cfg(test)]
mod tests {
    use super::resolve_stdio_executable;
    use std::collections::BTreeMap;

    #[test]
    fn keeps_explicit_path_command_unchanged() {
        let env = BTreeMap::new();
        let resolved = resolve_stdio_executable("./mock-server", false, &env);
        assert_eq!(resolved, "./mock-server");
    }

    #[test]
    fn resolves_command_from_path_when_available() {
        let unique = format!(
            "agentjax-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let dir = std::env::temp_dir().join(unique);
        std::fs::create_dir_all(&dir).expect("create temp dir");

        let executable_path = dir.join("mockcmd");
        std::fs::write(&executable_path, "#!/bin/sh\nexit 0\n").expect("write");

        let mut env = BTreeMap::new();
        env.insert("PATH".to_string(), dir.to_string_lossy().to_string());

        let resolved = resolve_stdio_executable("mockcmd", false, &env);
        assert_eq!(resolved, executable_path.to_string_lossy());

        let _ = std::fs::remove_file(&executable_path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
