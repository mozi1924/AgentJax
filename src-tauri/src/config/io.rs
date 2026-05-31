use crate::config::constants::CONFIG_FILE_NAME;
use crate::config::schema::AppConfig;
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigInfo {
    pub config_path: String,
    pub active_provider: String,
    pub provider_keys: Vec<String>,
    pub default_model: String,
    pub utility_small_model: String,
    pub models: Vec<String>,
    pub has_credential: bool,
    pub credential_env: String,
    pub request_timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigUpgradeResult {
    pub config_path: String,
    pub upgraded: bool,
}

pub fn config_dir_path() -> Result<PathBuf, String> {
    crate::agentjax_home::agentjax_home_dir()
}

pub fn init_config_if_missing() -> Result<PathBuf, String> {
    let dir = config_dir_path()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create config directory {}: {e}", dir.display()))?;
    }

    let path = dir.join(CONFIG_FILE_NAME);
    if !path.exists() {
        let template = default_config_yaml();
        fs::write(&path, template)
            .map_err(|e| format!("Failed to create config file {}: {e}", path.display()))?;
    }

    Ok(path)
}

pub fn load_config() -> Result<AppConfig, String> {
    register_home_provider_plugins();
    let path = init_config_if_missing()?;
    let raw = read_config_file(&path)?;
    let parsed = parse_config_yaml(&path, &raw)?;
    Ok(parsed.normalize())
}

pub fn upgrade_config_file() -> Result<ConfigUpgradeResult, String> {
    register_home_provider_plugins();
    let path = init_config_if_missing()?;
    let raw = read_config_file(&path)?;
    let parsed = parse_config_yaml(&path, &raw)?;
    let normalized = parsed.normalize();
    let upgraded = persist_config_if_changed(&path, &raw, &normalized)?;

    Ok(ConfigUpgradeResult {
        config_path: path.display().to_string(),
        upgraded,
    })
}

fn register_home_provider_plugins() {
    match crate::plugin_runtime::discover_home_plugin_packages() {
        Ok(packages) => {
            crate::provider_api::registry::register_plugin_providers_from_packages(packages)
        }
        Err(err) => log::warn!("Failed to discover provider plugins: {err}"),
    }
}

pub fn get_config_info() -> Result<ConfigInfo, String> {
    let path = init_config_if_missing()?;
    let config = load_config()?;
    let active_provider = config.active_provider.clone();
    let active_provider_config = config.resolved_provider(&active_provider)?;

    Ok(ConfigInfo {
        config_path: path.display().to_string(),
        active_provider,
        provider_keys: config.provider_keys(),
        default_model: config.default_model.clone(),
        utility_small_model: config.utility_small_model.clone(),
        models: config.configured_models(),
        has_credential: active_provider_config.resolved_credential().is_some(),
        credential_env: active_provider_config.credential_env(),
        request_timeout_seconds: config.request_timeout_seconds,
    })
}

fn default_config_yaml() -> String {
    let config = AppConfig::default();
    let mut lines = [
        "# AgentJax configuration".to_string(),
        "# Home directory: AGENTJAX_HOME (default: ~/.agentjax)".to_string(),
        "# Config path: $AGENTJAX_HOME/config.yaml".to_string(),
        "# Plugin directory: $AGENTJAX_HOME/plugins".to_string(),
        String::new(),
        serde_yaml::to_string(&config).expect("default AppConfig should serialize to YAML"),
    ]
    .to_vec();
    lines.push(String::new());
    lines.join("\n")
}

fn read_config_file(path: &Path) -> Result<String, String> {
    fs::read_to_string(path)
        .map_err(|e| format!("Failed to read config file {}: {e}", path.display()))
}

fn parse_config_yaml(path: &Path, raw: &str) -> Result<AppConfig, String> {
    serde_yaml::from_str(raw).map_err(|e| format!("Invalid YAML in {}: {e}", path.display()))
}

fn persist_config_if_changed(
    path: &Path,
    raw: &str,
    normalized: &AppConfig,
) -> Result<bool, String> {
    let source_value: serde_yaml::Value = serde_yaml::from_str(raw)
        .map_err(|e| format!("Invalid YAML in {}: {e}", path.display()))?;

    let normalized_yaml = serde_yaml::to_string(normalized)
        .map_err(|e| format!("Failed to serialize normalized config: {e}"))?;
    let normalized_value: serde_yaml::Value = serde_yaml::from_str(&normalized_yaml)
        .map_err(|e| format!("Failed to parse normalized config YAML: {e}"))?;

    if source_value == normalized_value {
        return Ok(false);
    }

    fs::write(path, normalized_yaml).map_err(|e| {
        format!(
            "Failed to write upgraded config file {}: {e}",
            path.display()
        )
    })?;
    log::info!("Config file upgraded at {}", path.display());

    Ok(true)
}
