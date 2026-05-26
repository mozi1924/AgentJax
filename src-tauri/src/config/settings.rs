use crate::config::settings_ui;
use crate::config::{self, AppConfig};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsOption {
    pub label: String,
    pub value: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SecretStatus {
    pub configured: bool,
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSnapshot {
    pub config_path: String,
    pub revision: String,
    pub values: Value,
    pub dynamic_options: BTreeMap<String, Vec<SettingsOption>>,
    pub secret_statuses: BTreeMap<String, SecretStatus>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingsPatchOperation {
    Set,
    Delete,
}

fn default_patch_operation() -> SettingsPatchOperation {
    SettingsPatchOperation::Set
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsPatch {
    pub path: String,
    #[serde(default)]
    pub value: Option<Value>,
    pub expected_revision: String,
    #[serde(default = "default_patch_operation")]
    pub operation: SettingsPatchOperation,
}

pub fn get_settings_snapshot() -> Result<SettingsSnapshot, String> {
    let config_path = config::init_config_if_missing()?;
    let raw = fs::read_to_string(&config_path)
        .map_err(|e| format!("Failed to read config file {}: {e}", config_path.display()))?;
    let config = config::load_config()?;
    snapshot_from_config(&config, &config_path, &raw)
}

pub fn get_settings_ui_snapshot() -> Result<settings_ui::SettingsUiSnapshot, String> {
    let snapshot = get_settings_snapshot()?;
    Ok(settings_ui::SettingsUiSnapshot {
        snapshot,
        sections: settings_ui::build_settings_sections()?,
    })
}

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

pub fn snapshot_from_config(
    config: &AppConfig,
    config_path: &Path,
    raw: &str,
) -> Result<SettingsSnapshot, String> {
    let mut values = serde_json::to_value(config)
        .map_err(|e| format!("Failed to serialize config snapshot: {e}"))?;
    let mut secret_statuses = BTreeMap::new();
    redact_secret_values(config, &mut values, &mut secret_statuses)?;

    Ok(SettingsSnapshot {
        config_path: config_path.display().to_string(),
        revision: compute_revision(raw),
        values,
        dynamic_options: settings_ui::build_dynamic_options(config)?,
        secret_statuses,
    })
}

fn redact_secret_values(
    config: &AppConfig,
    values: &mut Value,
    secret_statuses: &mut BTreeMap<String, SecretStatus>,
) -> Result<(), String> {
    let root = values
        .as_object_mut()
        .ok_or_else(|| "Config snapshot root is not an object".to_string())?;

    if let Some(Value::Object(providers)) = root.get_mut("providers") {
        for (provider_key, provider_value) in providers.iter_mut() {
            if let Value::Object(provider_object) = provider_value {
                provider_object.insert("credential".to_string(), Value::Null);
                let provider_config = config.providers.get(provider_key);
                let inline_credential = provider_config
                    .and_then(|entry| entry.credential.as_deref())
                    .map(str::trim)
                    .filter(|value| !value.is_empty())
                    .is_some();
                let env_credential = provider_config
                    .and_then(|entry| {
                        std::env::var(&entry.credential_env)
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

    if let Some(Value::Object(servers)) = root.get_mut("mcp_servers") {
        for (server_key, server_value) in servers.iter_mut() {
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

    Ok(())
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

fn atomic_write(path: &Path, content: &str) -> Result<(), String> {
    let temp_path = temp_config_path(path);
    fs::write(&temp_path, content).map_err(|e| {
        format!(
            "Failed to write temporary config file {}: {e}",
            temp_path.display()
        )
    })?;
    fs::rename(&temp_path, path).map_err(|e| {
        format!(
            "Failed to replace config file {} with {}: {e}",
            path.display(),
            temp_path.display()
        )
    })?;
    Ok(())
}

fn temp_config_path(path: &Path) -> PathBuf {
    let mut temp = path.to_path_buf();
    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.yaml");
    temp.set_file_name(format!("{}.tmp", file_name));
    temp
}

fn compute_revision(raw: &str) -> String {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in raw.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{:016x}", hash)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agentjax_home::AGENTJAX_HOME_ENV;

    fn write_test_config(home: &Path) -> std::path::PathBuf {
        let path = home.join("config.yaml");
        let raw = [
            "active_provider: \"openai\"",
            "default_model: \"openai/gpt-5-mini\"",
            "utility_small_model: \"openai/gpt-5-mini\"",
            "request_timeout_seconds: 120",
            "system_prompt: \"Assistant\"",
            "providers:",
            "  openai:",
            "    kind: \"openai\"",
            "    api_endpoint: \"https://api.openai.com/v1\"",
            "    realtime_endpoint: \"\"",
            "    stream_transport: \"websocket\"",
            "    credential: \"SECRET\"",
            "    credential_env: \"OPENAI_API_KEY\"",
            "    request_timeout_seconds: 120",
            "    models:",
            "      gpt-5-mini:",
            "        model: \"gpt-5-mini\"",
            "        enabled: true",
            "        request:",
            "          reasoning_effort: null",
            "          extra_body: {}",
            "mcp_runtime:",
            "  stdio:",
            "    inherit_parent_env: false",
            "    env: {}",
            "  startup_timeout_ms: 15000",
            "  tool_timeout_ms: 60000",
            "mcp_servers: {}",
            "",
        ]
        .join("\n");
        fs::write(&path, raw).expect("write config");
        path
    }

    #[test]
    fn snapshot_redacts_secret_values() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home =
            std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create home");
        let _path = write_test_config(&home);

        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }

        let snapshot = get_settings_snapshot().expect("snapshot");
        assert_eq!(
            snapshot.values["providers"]["openai"]["credential"],
            Value::Null
        );
        assert_eq!(
            snapshot
                .secret_statuses
                .get("providers.openai.credential")
                .expect("secret status")
                .source,
            "inline"
        );

        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn apply_patch_updates_scalar_values() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home =
            std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create home");
        let path = write_test_config(&home);

        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }

        let snapshot = get_settings_snapshot().expect("snapshot");
        let updated = apply_settings_patch(SettingsPatch {
            path: "request_timeout_seconds".to_string(),
            value: Some(Value::from(33)),
            expected_revision: snapshot.revision,
            operation: SettingsPatchOperation::Set,
        })
        .expect("apply patch");

        assert_eq!(updated.values["request_timeout_seconds"], Value::from(33));
        let raw = fs::read_to_string(&path).expect("read config");
        assert!(raw.contains("request_timeout_seconds: 33"));

        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn apply_patch_rejects_invalid_collection_keys() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home =
            std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create home");
        let _path = write_test_config(&home);

        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }

        let snapshot = get_settings_snapshot().expect("snapshot");
        let error = apply_settings_patch(SettingsPatch {
            path: "mcp_servers.bad$key".to_string(),
            value: Some(serde_json::json!({ "transport": "stdio", "enabled": true })),
            expected_revision: snapshot.revision,
            operation: SettingsPatchOperation::Set,
        })
        .expect_err("invalid key should fail");
        assert!(error.contains("unsupported characters"));

        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn apply_patch_supports_escaped_model_profile_keys_with_dots() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home =
            std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create home");
        let path = write_test_config(&home);

        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }

        let snapshot = get_settings_snapshot().expect("snapshot");
        let updated = apply_settings_patch(SettingsPatch {
            path: "providers.openai.models.GPT-5\\.4.model".to_string(),
            value: Some(Value::from("gpt-5.4")),
            expected_revision: snapshot.revision,
            operation: SettingsPatchOperation::Set,
        })
        .expect("apply patch with escaped model profile key");

        assert_eq!(
            updated.values["providers"]["openai"]["models"]["GPT-5.4"]["model"],
            Value::from("gpt-5.4")
        );
        let raw = fs::read_to_string(&path).expect("read config");
        assert!(raw.contains("GPT-5.4:"));

        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = fs::remove_dir_all(home);
    }
}
