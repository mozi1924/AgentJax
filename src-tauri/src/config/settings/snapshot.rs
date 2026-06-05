use super::io::compute_revision;
use super::types::{SecretStatus, SettingsSnapshot};
use crate::agentjax_err;
use crate::config::agent_config::AgentConfig;
use crate::config::settings_ui;
use crate::config::{self, AppConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

/// Agent-specific config paths that should be read/written from agent.yaml.
/// All other paths are shared and go to config.yaml.
const AGENT_CONFIG_PATHS: &[&str] = &[
    "active_provider",
    "default_model",
    "utility_small_model",
    "request_timeout_seconds",
    "show_advanced_request_options",
    "enable_developer_tools",
    "prompt_composer",
    "context_management",
    "sub_agent",
    "memory",
    "rag",
    "tool_manager",
];

/// Determine whether a settings path belongs to the agent config or shared config.
pub fn is_agent_config_path(path: &str) -> bool {
    let root_segment = path.split('.').next().unwrap_or("");
    AGENT_CONFIG_PATHS.contains(&root_segment)
}

/// Get the effective agent_id for settings (from argument or default).
fn resolve_settings_agent_id(agent_id: Option<&str>) -> String {
    agent_id
        .filter(|s| !s.is_empty())
        .unwrap_or(crate::config::constants::DEFAULT_AGENT_ID)
        .to_string()
}

pub fn get_settings_snapshot(agent_id: Option<&str>) -> AgentJaxResult<SettingsSnapshot> {
    let agent_id = resolve_settings_agent_id(agent_id);

    let config_path = config::init_config_if_missing()?;
    let raw = fs::read_to_string(&config_path).map_err(|e| {
        AgentJaxError::config(format!(
            "Failed to read config file {}: {e}",
            config_path.display()
        ))
        .with_error_source(&e)
    })?;
    let config = config::load_config()?;
    let agent_config = config::load_agent_config(&agent_id)?;

    snapshot_from_config_with_agent(&config, &agent_config, &config_path, &raw)
}

pub fn get_settings_ui_snapshot(
    agent_id: Option<&str>,
) -> AgentJaxResult<settings_ui::SettingsUiSnapshot> {
    let snapshot = get_settings_snapshot(agent_id)?;
    Ok(settings_ui::SettingsUiSnapshot {
        snapshot,
        sections: settings_ui::build_settings_sections()?,
    })
}

pub(super) fn snapshot_from_config_with_agent(
    config: &AppConfig,
    agent_config: &AgentConfig,
    config_path: &Path,
    raw: &str,
) -> AgentJaxResult<SettingsSnapshot> {
    let mut values = serde_json::to_value(config).map_err(|e| {
        AgentJaxError::config(format!("Failed to serialize config snapshot: {e}"))
            .with_error_source(&e)
    })?;

    // Merge agent config fields into the values object so the frontend
    // sees a unified view of both shared and agent-specific settings.
    if let Ok(agent_values) = serde_json::to_value(agent_config) {
        if let (Some(root_obj), Some(agent_obj)) =
            (values.as_object_mut(), agent_values.as_object())
        {
            for (key, val) in agent_obj {
                // Agent values override shared values for the same key.
                root_obj.insert(key.clone(), val.clone());
            }
        }
    }

    let mut secret_statuses = BTreeMap::new();
    redact_secret_values(config, &mut values, &mut secret_statuses)?;

    Ok(SettingsSnapshot {
        config_path: config_path.display().to_string(),
        revision: compute_revision(raw),
        values,
        dynamic_options: config::build_dynamic_options(config)?,
        secret_statuses,
    })
}

fn redact_secret_values(
    config: &AppConfig,
    values: &mut Value,
    secret_statuses: &mut BTreeMap<String, SecretStatus>,
) -> AgentJaxResult<()> {
    let root = values
        .as_object_mut()
        .ok_or_else(|| agentjax_err!("Config snapshot root is not an object", Config))?;

    if let Some(Value::Object(providers)) = root.get_mut("providers") {
        for (provider_key, provider_value) in providers.iter_mut() {
            if let Value::Object(provider_object) = provider_value {
                provider_object.insert("credential".to_string(), Value::Null);
                let provider_config = config.providers.get(provider_key);
                let inline_credential = provider_config
                    .and_then(|entry| entry.credential())
                    .map(|v| v.trim().to_string())
                    .filter(|value| !value.is_empty())
                    .is_some();
                let env_credential = provider_config
                    .and_then(|entry| {
                        std::env::var(entry.credential_env())
                            .ok()
                            .map(|value| value.trim().to_string())
                    })
                    .filter(|value| !value.is_empty())
                    .is_some();
                let source = if inline_credential {
                    "inline"
                } else if env_credential {
                    "env"
                } else {
                    "unset"
                };
                secret_statuses.insert(
                    format!("providers.{provider_key}.credential"),
                    SecretStatus {
                        configured: inline_credential || env_credential,
                        source: source.to_string(),
                    },
                );
            }
        }
    }

    if let Some(mcp_value) = root.get_mut("mcp") {
        if let Some(servers_map) = mcp_value.get_mut("servers").and_then(|s| s.as_object_mut()) {
            for (server_key, server_value) in servers_map.iter_mut() {
                if let Value::Object(server_object) = server_value {
                    let configured = server_object
                        .get("auth_header")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .is_some();
                    server_object.insert("auth_header".to_string(), Value::Null);
                    secret_statuses.insert(
                        format!("mcp_servers.{server_key}.auth_header"),
                        SecretStatus {
                            configured,
                            source: if configured { "inline" } else { "unset" }.to_string(),
                        },
                    );
                }
            }
        }
    }

    Ok(())
}
