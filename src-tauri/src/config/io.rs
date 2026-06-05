use crate::config::agent_config::{AgentConfig, AgentRegistry, FullConfig};
use crate::config::constants::CONFIG_FILE_NAME;
use crate::config::schema::AppConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
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

pub fn config_dir_path() -> AgentJaxResult<PathBuf> {
    crate::agentjax_home::agentjax_home_dir()}

pub fn init_config_if_missing() -> AgentJaxResult<PathBuf> {
    let dir = config_dir_path()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| AgentJaxError::config(format!("Failed to create config directory {}: {e}", dir.display())).with_error_source(&e))?;
    }

    let path = dir.join(CONFIG_FILE_NAME);
    if !path.exists() {
        let template = default_config_yaml();
        fs::write(&path, template)
            .map_err(|e| AgentJaxError::config(format!("Failed to create config file {}: {e}", path.display())).with_error_source(&e))?;
    }

    Ok(path)
}

pub fn load_config() -> AgentJaxResult<AppConfig> {
    register_home_provider_plugins();
    let path = init_config_if_missing()?;
    let raw = read_config_file(&path)?;
    let parsed = parse_config_yaml(&path, &raw)?;
    Ok(parsed.normalize())
}

/// Load an agent-specific configuration from `~/.agentjax/agents/{agent_id}/agent.yaml`.
/// Returns the default `AgentConfig` if the file doesn't exist yet.
pub fn load_agent_config(agent_id: &str) -> AgentJaxResult<AgentConfig> {
    let registry = AgentRegistry::new()?;
    if registry.agent_exists(agent_id) {
        registry.load_agent_config(agent_id)
    } else {
        // Return a default that inherits from the shared config.yaml fields
        // for any values that were historically stored there.
        Ok(AgentConfig::default().normalize())
    }
}

/// Load the shared config + agent config and merge into a `FullConfig`.
pub fn load_full_config(agent_id: &str) -> AgentJaxResult<FullConfig> {
    let shared = load_config()?;
    let agent = load_agent_config(agent_id)?;
    Ok(FullConfig::new(shared, agent, agent_id.to_string()))
}

/// Ensure the default agent profile exists on disk.
/// Writes a default `agent.yaml` from the existing `AppConfig` fields for migration.
pub fn ensure_default_agent_profile() -> AgentJaxResult<()> {
    crate::agentjax_home::ensure_default_agent()?;
    Ok(())
}

pub fn upgrade_config_file() -> AgentJaxResult<ConfigUpgradeResult> {
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

pub fn get_config_info() -> AgentJaxResult<ConfigInfo> {
    let path = init_config_if_missing()?;
    let config = load_config()?;
    // Attempt to read the active agent's config for per-agent defaults.
    let agent = load_agent_config(&config.active_agent_id).unwrap_or_default().normalize();
    let active_provider = agent.active_provider.clone();
    let active_provider_config = config.resolved_provider(&active_provider)?;

    Ok(ConfigInfo {
        config_path: path.display().to_string(),
        active_provider,
        provider_keys: config.provider_keys(),
        default_model: agent.default_model.clone(),
        utility_small_model: agent.utility_small_model.clone(),
        models: config.configured_models(),
        has_credential: active_provider_config.resolved_credential().is_some(),
        credential_env: active_provider_config.credential_env(),
        request_timeout_seconds: agent.request_timeout_seconds,
    })
}

fn default_config_yaml() -> String {
    let config = AppConfig::default();
    let yaml_body = serialize_config_to_yaml(&config)
        .unwrap_or_else(|e| {
            panic!("Failed to serialize default config to YAML: {e}")
        });
    let mut lines = [
        "# AgentJax configuration".to_string(),
        "# Home directory: AGENTJAX_HOME (default: ~/.agentjax)".to_string(),
        "# Config path: $AGENTJAX_HOME/config.yaml".to_string(),
        "# Plugin directory: $AGENTJAX_HOME/plugins".to_string(),
        String::new(),
        yaml_body,
    ]
    .to_vec();
    lines.push(String::new());
    lines.join("\n")
}

fn read_config_file(path: &Path) -> AgentJaxResult<String> {
    fs::read_to_string(path)
        .map_err(|e| AgentJaxError::config(format!("Failed to read config file {}: {e}", path.display())).with_error_source(&e))
}

fn parse_config_yaml(path: &Path, raw: &str) -> AgentJaxResult<AppConfig> {
    serde_yaml::from_str(raw).map_err(|e| AgentJaxError::config(format!("Invalid YAML in {}: {e}", path.display())).with_error_source(&e))
}

pub fn serialize_config_to_yaml(normalized: &AppConfig) -> AgentJaxResult<String> {
    // Prompt composer blocks are now per-agent; the shared config.yaml does not
    // contain them. Future YAML abbreviation logic should go here if needed.
    serde_yaml::to_string(normalized)
        .map_err(|e| AgentJaxError::config(format!("Failed to serialize normalized config to YAML: {e}")).with_error_source(&e))
}

fn persist_config_if_changed(
    path: &Path,
    raw: &str,
    normalized: &AppConfig,
) -> AgentJaxResult<bool> {
    let source_value: serde_yaml::Value = serde_yaml::from_str(raw)
        .map_err(|e| AgentJaxError::config(format!("Invalid YAML in {}: {e}", path.display())).with_error_source(&e))?;

    let normalized_yaml = serialize_config_to_yaml(normalized)?;
    let normalized_value: serde_yaml::Value = serde_yaml::from_str(&normalized_yaml)
        .map_err(|e| AgentJaxError::config(format!("Failed to parse normalized config YAML: {e}")).with_error_source(&e))?;

    if source_value == normalized_value {
        return Ok(false);
    }

    fs::write(path, normalized_yaml).map_err(|e| {
        AgentJaxError::config(format!(
            "Failed to write upgraded config file {}: {e}",
            path.display()
        ))
        .with_error_source(&e)
    })?;
    log::info!("Config file upgraded at {}", path.display());

    Ok(true)
}
