use super::io::{atomic_write, compute_revision};
use super::snapshot::snapshot_from_config;
use super::types::{SettingsPatch, SettingsPatchOperation, SettingsSnapshot};
use crate::agentjax_err;
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

    let config = config::load_config()?;
    let mut root = serde_json::to_value(&config)
        .map_err(|e| AgentJaxError::config(format!("Failed to serialize current config for patching: {e}")).with_error_source(&e))?;

    let path_segments = parse_path(&patch.path)?;
    match patch.operation {
        SettingsPatchOperation::Set => {
            let value = patch
                .value
                .ok_or_else(|| agentjax_err!(format!("Patch for '{}' requires a value", patch.path), Config))?;
            apply_set(&mut root, &path_segments, value)?;
        }
        SettingsPatchOperation::Delete => {
            apply_delete(&mut root, &path_segments)?;
        }
    }

    validate_path_semantics(&path_segments, &root)?;

    let patched: AppConfig = serde_json::from_value(root)
        .map_err(|e| AgentJaxError::config(format!("Patched configuration is invalid: {e}")).with_error_source(&e))?;
    let normalized = patched.normalize();
    let normalized_yaml = crate::config::serialize_config_to_yaml(&normalized)?;

    atomic_write(&config_path, &normalized_yaml)?;
    snapshot_from_config(&normalized, &config_path, &normalized_yaml)
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
    if segments.is_empty() {
        return Ok(());
    }

    if segments[0] == "providers" && segments.len() >= 2 {
        validate_key(&segments[1], "provider key")?;
        if segments.len() >= 4 && segments[2] == "models" {
            validate_key(&segments[3], "model profile key")?;
        }
    }

    if segments[0] == "mcp" && segments.len() >= 3 && segments[1] == "servers" {
        validate_key(&segments[1], "MCP server key")?;
    }

    if segments[0] == "tool_manager" {
        validate_tool_manager_path(segments)?;
    }

    if let Some(Value::Object(providers)) = root.get("providers") {
        for provider_key in providers.keys() {
            validate_key(provider_key, "provider key")?;
        }
    }

    if let Some(mcp_value) = root.get("mcp") {
        if let Some(servers_map) = mcp_value.get("servers").and_then(|s| s.as_object()) {
            for server_key in servers_map.keys() {
                validate_key(server_key, "MCP server key")?;
            }
        }
    }

    if let Some(Value::Object(tool_manager)) = root.get("tool_manager") {
        validate_tool_manager_keys(tool_manager)?;
    }

    Ok(())
}

fn validate_tool_manager_path(segments: &[String]) -> AgentJaxResult<()> {
    if segments.len() >= 3 {
        match segments[1].as_str() {
            "native_tools" => validate_key(&segments[2], "native tool key")?,
            "context_tools" => validate_key(&segments[2], "context tool key")?,
            "plugin_tools" => validate_key(&segments[2], "plugin id")?,
            "mcp_tools" => validate_key(&segments[2], "MCP server key")?,
            _ => {}
        }
    }
    if segments.len() >= 5 && segments[3] == "tools" {
        validate_key(&segments[4], "tool key")?;
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
        for (source_key, source_value) in sources {
            validate_key(source_key, label)?;
            let Some(source_object) = source_value.as_object() else {
                continue;
            };
            let Some(Value::Object(tools)) = source_object.get("tools") else {
                continue;
            };
            for tool_key in tools.keys() {
                validate_key(tool_key, "tool key")?;
            }
        }
    }
    Ok(())
}

fn validate_key(key: &str, label: &str) -> AgentJaxResult<()> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err(agentjax_err!(format!("{label} cannot be empty"), Config));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        return Err(agentjax_err!(
            format!("{label} '{}' contains unsupported characters. Use letters, digits, '-', '_' or '.' only.", trimmed),
            Config
        ));
    }
    Ok(())
}
