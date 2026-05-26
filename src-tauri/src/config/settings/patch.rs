use super::io::{atomic_write, compute_revision};
use super::snapshot::snapshot_from_config;
use super::types::{SettingsPatch, SettingsPatchOperation, SettingsSnapshot};
use crate::config::{self, AppConfig};
use serde_json::{Map, Value};
use std::fs;

pub fn apply_settings_patch(patch: SettingsPatch) -> Result<SettingsSnapshot, String> {
    let config_path = config::init_config_if_missing()?;
    let raw = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config file {}: {e}", config_path.display()))?;
    let current_revision = compute_revision(&raw);

    if current_revision != patch.expected_revision {
        return Err(
            "Configuration changed on disk. Please reload settings and try again.".to_string(),
        );
    }

    let config = config::load_config()?;
    let mut root = serde_json::to_value(&config)
        .map_err(|e| format!("Failed to serialize current config for patching: {e}"))?;

    let path_segments = parse_path(&patch.path)?;
    match patch.operation {
        SettingsPatchOperation::Set => {
            let value = patch
                .value
                .ok_or_else(|| format!("Patch for '{}' requires a value", patch.path))?;
            apply_set(&mut root, &path_segments, value)?;
        }
        SettingsPatchOperation::Delete => {
            apply_delete(&mut root, &path_segments)?;
        }
    }

    validate_path_semantics(&path_segments, &root)?;

    let patched: AppConfig = serde_json::from_value(root)
        .map_err(|e| format!("Patched configuration is invalid: {e}"))?;
    let normalized = patched.normalize();
    let normalized_yaml = serde_yaml::to_string(&normalized)
        .map_err(|e| format!("Failed to serialize normalized config: {e}"))?;

    atomic_write(&config_path, &normalized_yaml)?;
    snapshot_from_config(&normalized, &config_path, &normalized_yaml)
}

fn parse_path(path: &str) -> Result<Vec<String>, String> {
    let trimmed = path.trim();
    if trimmed.is_empty() {
        return Err("Patch path cannot be empty".to_string());
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
                    return Err(format!("Patch path '{}' contains an empty segment", path));
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
        return Err(format!("Patch path '{}' contains an empty segment", path));
    }
    segments.push(last_segment);

    if segments.iter().any(|segment| segment.is_empty()) {
        return Err(format!("Patch path '{}' contains an empty segment", path));
    }

    Ok(segments)
}

fn apply_set(root: &mut Value, segments: &[String], value: Value) -> Result<(), String> {
    if segments.is_empty() {
        *root = value;
        return Ok(());
    }

    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        let object = current
            .as_object_mut()
            .ok_or_else(|| format!("Path segment '{}' does not reference an object", segment))?;
        current = object
            .entry(segment.clone())
            .or_insert_with(|| Value::Object(Map::new()));
    }

    let leaf = segments
        .last()
        .ok_or_else(|| "Patch path missing terminal segment".to_string())?;
    let object = current
        .as_object_mut()
        .ok_or_else(|| format!("Path '{}' does not reference an object leaf", leaf))?;
    object.insert(leaf.clone(), value);
    Ok(())
}

fn apply_delete(root: &mut Value, segments: &[String]) -> Result<(), String> {
    if segments.is_empty() {
        return Err("Delete patch path cannot be empty".to_string());
    }

    let mut current = root;
    for segment in &segments[..segments.len() - 1] {
        let object = current
            .as_object_mut()
            .ok_or_else(|| format!("Path segment '{}' does not reference an object", segment))?;
        current = object
            .get_mut(segment)
            .ok_or_else(|| format!("Path segment '{}' does not exist", segment))?;
    }

    let leaf = segments
        .last()
        .ok_or_else(|| "Delete path missing terminal segment".to_string())?;
    let object = current
        .as_object_mut()
        .ok_or_else(|| format!("Cannot delete '{}' from a non-object parent", leaf))?;
    object.remove(leaf);
    Ok(())
}

fn validate_path_semantics(segments: &[String], root: &Value) -> Result<(), String> {
    if segments.is_empty() {
        return Ok(());
    }

    if segments[0] == "providers" && segments.len() >= 2 {
        validate_key(&segments[1], "provider key")?;
        if segments.len() >= 4 && segments[2] == "models" {
            validate_key(&segments[3], "model profile key")?;
        }
    }

    if segments[0] == "mcp_servers" && segments.len() >= 2 {
        validate_key(&segments[1], "MCP server key")?;
    }

    if let Some(Value::Object(providers)) = root.get("providers") {
        for provider_key in providers.keys() {
            validate_key(provider_key, "provider key")?;
        }
    }

    if let Some(Value::Object(servers)) = root.get("mcp_servers") {
        for server_key in servers.keys() {
            validate_key(server_key, "MCP server key")?;
        }
    }

    Ok(())
}

fn validate_key(key: &str, label: &str) -> Result<(), String> {
    let trimmed = key.trim();
    if trimmed.is_empty() {
        return Err(format!("{label} cannot be empty"));
    }
    if !trimmed
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' || ch == '.')
    {
        return Err(format!(
            "{label} '{}' contains unsupported characters. Use letters, digits, '-', '_' or '.' only.",
            trimmed
        ));
    }
    Ok(())
}
