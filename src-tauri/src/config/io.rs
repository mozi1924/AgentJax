use crate::config::constants::{
    BUILTIN_CORE_SYSTEM_BLOCK_CONTENT, BUILTIN_CORE_SYSTEM_BLOCK_ID, BUILTIN_CORE_SYSTEM_SOURCE_ID,
    BUILTIN_CORE_SYSTEM_TITLE, CONFIG_FILE_NAME, DEFAULT_DEFAULT_MODEL_REF,
    DEFAULT_TIMEOUT_SECONDS, DEFAULT_UTILITY_SMALL_MODEL_REF,
};
use crate::config::schema::{AppConfig, ProviderConfig};
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
    let path = init_config_if_missing()?;
    let raw = read_config_file(&path)?;
    let parsed = parse_config_yaml(&path, &raw)?;
    Ok(parsed.normalize())
}

pub fn upgrade_config_file() -> Result<ConfigUpgradeResult, String> {
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
    let provider = ProviderConfig::default();

    let mut lines = vec![
        "# AgentJax configuration".to_string(),
        "# Home directory: AGENTJAX_HOME (default: ~/.agentjax)".to_string(),
        "# Config path: $AGENTJAX_HOME/config.yaml".to_string(),
        "# Plugin directory: $AGENTJAX_HOME/plugins".to_string(),
        String::new(),
        format!("active_provider: \"{}\"", provider.kind),
        format!("default_model: \"{}\"", DEFAULT_DEFAULT_MODEL_REF),
        format!(
            "utility_small_model: \"{}\"",
            DEFAULT_UTILITY_SMALL_MODEL_REF
        ),
        format!("request_timeout_seconds: {}", DEFAULT_TIMEOUT_SECONDS),
        "show_advanced_request_options: false".to_string(),
        "enable_developer_tools: false".to_string(),
        "language: \"auto\"".to_string(),
        "prompt_composer:".to_string(),
        "  blocks:".to_string(),
        format!("    - id: \"{}\"", BUILTIN_CORE_SYSTEM_BLOCK_ID),
        format!("      title: \"{}\"", BUILTIN_CORE_SYSTEM_TITLE),
        "      role: \"system\"".to_string(),
        "      enabled: true".to_string(),
        "      source: \"builtin\"".to_string(),
        format!("      source_id: \"{}\"", BUILTIN_CORE_SYSTEM_SOURCE_ID),
        "      locked: true".to_string(),
        "      content: |".to_string(),
        indent_block(BUILTIN_CORE_SYSTEM_BLOCK_CONTENT, 8),
        String::new(),
        "providers:".to_string(),
        format!("  {}:", provider.kind),
        format!("    kind: \"{}\"", provider.kind),
        format!("    apiEndpoint: \"{}\"", provider.api_endpoint()),
        "    queryParams: {}".to_string(),
        "    httpHeaders: {}".to_string(),
        "    envHttpHeaders: {}".to_string(),
        "    realtimeEndpoint: \"\"".to_string(),
        format!("    supportsWebsockets: {}", provider.supports_websockets()),
        format!("    streamTransport: \"{}\"", provider.stream_transport()),
        "    credential: \"\"".to_string(),
        format!("    credentialEnv: \"{}\"", provider.credential_env()),
        "    requestTimeoutSeconds: 120".to_string(),
        "    requestMaxRetries: null".to_string(),
        "    streamMaxRetries: null".to_string(),
        "    streamIdleTimeoutMs: null".to_string(),
        "    websocketConnectTimeoutMs: null".to_string(),
        "    models:".to_string(),
    ];

    for model_key in provider.models.keys() {
        lines.push(format!("      {}:", model_key));
        lines.push(format!("        model: \"{}\"", model_key));
        lines.push("        enabled: true".to_string());
        lines.push("        request:".to_string());
        lines.push("          temperature: null".to_string());
        lines.push("          top_p: null".to_string());
        lines.push("          top_k: null".to_string());
        lines.push("          max_output_tokens: null".to_string());
        lines.push("          frequency_penalty: null".to_string());
        lines.push("          presence_penalty: null".to_string());
        lines.push("          reasoning_effort: null".to_string());
        lines.push("          extra_body: {}".to_string());
    }

    lines.extend([
        String::new(),
        "mcp_runtime:".to_string(),
        "  stdio:".to_string(),
        "    inherit_parent_env: false".to_string(),
        "    env: {}".to_string(),
        String::new(),
        "mcp_servers: {}".to_string(),
        String::new(),
        "tool_manager:".to_string(),
        "  native_tools: {}".to_string(),
        "  plugin_tools: {}".to_string(),
        "  mcp_tools: {}".to_string(),
        String::new(),
    ]);

    lines.join("\n")
}

fn indent_block(value: &str, spaces: usize) -> String {
    let indent = " ".repeat(spaces);
    value
        .lines()
        .map(|line| format!("{indent}{line}"))
        .collect::<Vec<_>>()
        .join("\n")
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
