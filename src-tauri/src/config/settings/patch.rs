use super::io::{atomic_write, compute_revision};
use super::snapshot::{
    is_agent_config_path, snapshot_from_config_with_agent,
};
use super::types::{SettingsPatch, SettingsPatchOperation, SettingsSnapshot};
use crate::agentjax_err;
use crate::config::agent_config::AgentConfig;
use crate::config::{self, AppConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use serde_json::{Map, Value};
use std::fs;

pub fn apply_settings_patch(patch: SettingsPatch) -> AgentJaxResult<SettingsSnapshot> {
    let config_path = config::init_config_if_missing()?;
    let raw = fs::read_to_string(&config_path)
        .map_err(|e| AgentJaxError::config(format!("Failed to read config file {}: {e}", config_path.display())).with_error_source(&e))?;
    let current_revision = compute_revision(&raw);

    if current_revision != patch.expected_revision {
        return Err(agentjax_err!(
            "Configuration changed on disk. Please reload settings and try again.",
            Config
        ));
    }

    let agent_id = patch
        .agent_id
        .as_deref()
        .unwrap_or(crate::config::constants::DEFAULT_AGENT_ID);
    let config = config::load_config()?;
    let agent_config = config::load_agent_config(agent_id)?;

    // Determine if this patch targets agent-specific or shared config.
    let is_agent_path = is_agent_config_path(&patch.path);

    if is_agent_path {
        apply_agent_patch(&patch, &config, &agent_config, agent_id)
    } else {
        apply_shared_patch(&patch, &config, &agent_config, &config_path, &raw)
    }
}

/// Apply a patch to the shared config.yaml.
fn apply_shared_patch(
    patch: &SettingsPatch,
    config: &AppConfig,
    agent_config: &AgentConfig,
    config_path: &std::path::Path,
    _raw: &str,
) -> AgentJaxResult<SettingsSnapshot> {
    let mut root = serde_json::to_value(config)
        .map_err(|e| AgentJaxError::config(format!("Failed to serialize config for patching: {e}")).with_error_source(&e))?;
    let path_segments = parse_path(&patch.path)?;

    match patch.operation {
        SettingsPatchOperation::Set => {
            let value = patch.value.clone().ok_or_else(|| {
                agentjax_err!(format!("Patch for '{}' requires a value", patch.path), Config)
            })?;
            apply_set(&mut root, &path_segments, value)?;
        }
        SettingsPatchOperation::Delete => {
            apply_delete(&mut root, &path_segments)?;
        }
    }
    validate_path_semantics(&path_segments, &root)?;

    let patched: AppConfig = serde_json::from_value(root)
        .map_err(|e| AgentJaxError::config(format!("Patched config is invalid: {e}")).with_error_source(&e))?;
    let normalized = patched.normalize();
    let normalized_yaml = crate::config::serialize_config_to_yaml(&normalized)?;
    atomic_write(config_path, &normalized_yaml)?;

    snapshot_from_config_with_agent(&normalized, agent_config, config_path, &normalized_yaml)
}

/// Apply a patch to the agent-specific agent.yaml.
fn apply_agent_patch(
    patch: &SettingsPatch,
    config: &AppConfig,
    agent_config: &AgentConfig,
    agent_id: &str,
) -> AgentJaxResult<SettingsSnapshot> {
    let agent_path = crate::agentjax_home::agent_dir(agent_id)?.join(crate::config::constants::AGENT_CONFIG_FILE_NAME);
    let mut root = serde_json::to_value(agent_config)
        .map_err(|e| AgentJaxError::config(format!("Failed to serialize agent config for patching: {e}")).with_error_source(&e))?;
    let path_segments = parse_path(&patch.path)?;

    match patch.operation {
        SettingsPatchOperation::Set => {
            let value = patch.value.clone().ok_or_else(|| {
                agentjax_err!(format!("Patch for '{}' requires a value", patch.path), Config)
            })?;
            apply_set(&mut root, &path_segments, value)?;
        }
        SettingsPatchOperation::Delete => {
            apply_delete(&mut root, &path_segments)?;
        }
    }

    let patched: AgentConfig = serde_json::from_value(root)
        .map_err(|e| AgentJaxError::config(format!("Patched agent config is invalid: {e}")).with_error_source(&e))?;
    let normalized = patched.normalize();
    let yaml = serde_yaml::to_string(&normalized)
        .map_err(|e| AgentJaxError::config(format!("Failed to serialize agent config: {e}")).with_error_source(&e))?;

    // Ensure the agent directory exists before writing.
    if let Some(parent) = agent_path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    std::fs::write(&agent_path, &yaml)
        .map_err(|e| AgentJaxError::config(format!("Failed to write agent config: {e}")).with_error_source(&e))?;

    let config_path = config::init_config_if_missing()?;
    let raw = fs::read_to_string(&config_path).unwrap_or_default();
    snapshot_from_config_with_agent(config, &normalized, &config_path, &raw)
}

fn parse_path(path: &str) -> AgentJaxResult<Vec<String>> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err(agentjax_err!("Patch path cannot be empty", Config));
    }

    let mut segments = Vec::new();
    let mut current = String::new();
    let mut escaped = false;

    for ch in trimmed.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }

        match ch {
            '\\' => escaped = true,
            '.' => {
                let segment = current.trim().to_string();
                if segment.is_empty() {
                    return Err(agentjax_err!(format!("Patch path '{}' contains an empty segment", path), Config));
                }
                segments.push(segment);
                current.clear();
            }
            _ => current.push(ch),
        }
    }

    if escaped {
        current.push('\\');
    }

    let last_segment = current.trim().to_string();
    if last_segment.is_empty() {
        return Err(agentjax_err!(format!("Patch path '{}' contains an empty segment", path), Config));
    }
    segments.push(last_segment);

    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(agentjax_err!(format!("Patch path '{}' contains an empty segment", path), Config));
    }

    Ok(segments)
}

fn apply_set(root: &mut Value, segments: &[String], value: Value) -> AgentJaxResult<()> {
    if segments.is_empty() {
        *root = value;
        return Ok(());
    }

    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        let object = current
            .as_object_mut()
            .ok_or_else(|| agentjax_err!(format!("Path segment '{}' does not reference an object", segment), Config))?;
        current = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }

    let leaf = segments
        .last()
        .ok_or_else(|| agentjax_err!("Patch path missing terminal segment", Config))?;
    let object = current
        .as_object_mut()
        .ok_or_else(|| agentjax_err!(format!("Path '{}' does not reference an object leaf", leaf), Config))?;
    object.insert(leaf.clone(), value);
    Ok(())
}

fn apply_delete(root: &mut Value, segments: &[String]) -> AgentJaxResult<()> {
    if segments.is_empty() {
        return Err(agentjax_err!("Delete patch path cannot be empty", Config));
    }

    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        let object = current
            .as_object_mut()
            .ok_or_else(|| agentjax_err!(format!("Path segment '{}' does not reference an object", segment), Config))?;
        current = object
            .get_mut(segment)
            .ok_or_else(|| agentjax_err!(format!("Path segment '{}' does not exist", segment), Config))?;
    }

    let leaf = segments
        .last()
        .ok_or_else(|| agentjax_err!("Delete path missing terminal segment", Config))?;
    let object = current
        .as_object_mut()
        .ok_or_else(|| agentjax_err!(format!("Cannot delete '{}' from a non-object parent", leaf), Config))?;
    object.remove(leaf);
    Ok(())
}

fn validate_path_semantics(segments: &[String], root: &Value) -> AgentJaxResult<()> {
    // Delegate to the unified schema-backed path registry.
    // This replaces the old ad-hoc validation with a single source of truth.
    crate::config::path_registry::validate_patch_path(segments, None)?;

    // Post-patch validation of collection keys in the result.
    // After applying the patch, check that all keys in the config match
    // the expected patterns (this catches invalid keys added by set operations).
    validate_root_keys(root)?;
    Ok(())
}

/// Post-patch key validation — ensures all collection keys in the final
/// config value match the expected format.
fn validate_root_keys(root: &Value) -> AgentJaxResult<()> {
    if let Some(Value::Object(providers)) = root.get("providers") {
        for provider_key in providers.keys() {
            crate::config::path_registry::validate_patch_path(
                &["providers".to_string(), provider_key.clone(), "enabled".to_string()],
                None,
            )?;
        }
    }

    if let Some(mcp_value) = root.get("mcp") {
        if let Some(servers_map) = mcp_value.get("servers").and_then(|s| s.as_object()) {
            for server_key in servers_map.keys() {
                crate::config::path_registry::validate_patch_path(
                    &["mcp".to_string(), "servers".to_string(), server_key.clone(), "enabled".to_string()],
                    None,
                )?;
            }
        }
    }

    if let Some(Value::Object(tool_manager)) = root.get("tool_manager") {
        validate_tool_manager_keys(tool_manager)?;
    }

    Ok(())
}

fn validate_tool_manager_keys(tool_manager: &Map<String, Value>) -> AgentJaxResult<()> {
    for (section, label) in [
        ("native_tools", "native tool key"),
        ("context_tools", "context tool key"),
        ("plugin_tools", "plugin id"),
        ("mcp_tools", "MCP server key"),
    ] {
        let Some(Value::Object(sources)) = tool_manager.get(section) else {
            continue;
        };
        for source_key in sources.keys() {
            crate::config::path_registry::validate_patch_path(
                &[
                    "tool_manager".to_string(),
                    section.to_string(),
                    source_key.clone(),
                    "enabled".to_string(),
                ],
                None,
            )?;
            let Some(source_object) = sources.get(source_key).and_then(|v| v.as_object()) else {
                continue;
            };
            let Some(Value::Object(tools)) = source_object.get("tools") else {
                continue;
            };
            for tool_key in tools.keys() {
                // Tool keys use a simpler validation since they're not full paths
                let trimmed = tool_key.trim();
                if trimmed.is_empty() {
                    return Err(agentjax_err!(format!("{label} tool key cannot be empty"), Config));
                }
            }
        }
    }
    Ok(())
}
