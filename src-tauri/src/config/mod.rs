use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const CONFIG_FILE_NAME: &str = "config.yaml";
const DEFAULT_SYSTEM_PROMPT: &str =
    "You are Codex, a helpful AI assistant. Follow the user's instructions.";
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_DEFAULT_MODEL_REF: &str = "openai/gpt-5-mini";
const DEFAULT_UTILITY_SMALL_MODEL_REF: &str = "openai/gpt-5-mini";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub active_provider: String,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub default_model: String,       // {provider}/{model_key}
    pub utility_small_model: String, // {provider}/{model_key}
    pub system_prompt: String,
    pub request_timeout_seconds: u64,
    #[serde(default)]
    pub mcp_runtime: McpRuntimeConfig,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub transport: McpTransportKind,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    #[serde(default = "default_true")]
    pub use_global_stdio_env: bool,
    pub inherit_parent_env: Option<bool>,
    pub uri: Option<String>,
    pub auth_header: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default = "default_true")]
    pub allow_stateless: bool,
    pub channel_buffer_capacity: Option<usize>,
    #[serde(default = "default_true")]
    pub reinit_on_expired_session: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct McpRuntimeConfig {
    pub stdio: McpStdioRuntimeConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpStdioRuntimeConfig {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub inherit_parent_env: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    #[default]
    Stdio,
    StreamableHttp,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: String,
    pub api_endpoint: String,
    #[serde(default)]
    pub models_endpoint_candidates: Vec<String>,
    pub realtime_endpoint: Option<String>,
    pub stream_transport: String,
    pub credential: Option<String>,
    pub credential_env: String,
    pub request_timeout_seconds: Option<u64>,
    #[serde(default)]
    pub models: BTreeMap<String, ProviderModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderModelConfig {
    pub model: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub request: ModelRequestConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelRequestConfig {
    pub temperature: Option<f32>,
    pub top_p: Option<f32>,
    pub top_k: Option<u32>,
    pub max_output_tokens: Option<u32>,
    pub frequency_penalty: Option<f32>,
    pub presence_penalty: Option<f32>,
    pub reasoning_effort: Option<String>,
    pub extra_body: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct ResolvedModelConfig {
    pub profile_key: String, // kept for compatibility in existing runtime structs
    pub provider_key: String,
    pub provider: ProviderConfig,
    pub model_id: String,
    pub model_ref: String,
    pub system_prompt: String,
    pub request: ModelRequestConfig,
    pub timeout_seconds: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert("openai".to_string(), ProviderConfig::default());

        Self {
            active_provider: "openai".to_string(),
            providers,
            default_model: DEFAULT_DEFAULT_MODEL_REF.to_string(),
            utility_small_model: DEFAULT_UTILITY_SMALL_MODEL_REF.to_string(),
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            request_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            mcp_runtime: McpRuntimeConfig::default(),
            mcp_servers: BTreeMap::new(),
        }
    }
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            transport: McpTransportKind::Stdio,
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            use_global_stdio_env: true,
            inherit_parent_env: None,
            uri: None,
            auth_header: None,
            headers: BTreeMap::new(),
            allow_stateless: true,
            channel_buffer_capacity: None,
            reinit_on_expired_session: true,
            enabled: true,
        }
    }
}

impl Default for McpStdioRuntimeConfig {
    fn default() -> Self {
        Self {
            env: BTreeMap::new(),
            inherit_parent_env: false,
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        let mut models = BTreeMap::new();
        models.insert(
            "gpt-5-mini".to_string(),
            ProviderModelConfig {
                model: "gpt-5-mini".to_string(),
                enabled: true,
                request: ModelRequestConfig::default(),
            },
        );
        models.insert(
            "gpt-5".to_string(),
            ProviderModelConfig {
                model: "gpt-5".to_string(),
                enabled: true,
                request: ModelRequestConfig::default(),
            },
        );

        Self {
            kind: "openai".to_string(),
            api_endpoint: "https://api.openai.com/v1".to_string(),
            models_endpoint_candidates: Vec::new(),
            realtime_endpoint: None,
            stream_transport: "websocket".to_string(),
            credential: None,
            credential_env: "OPENAI_API_KEY".to_string(),
            request_timeout_seconds: None,
            models,
        }
    }
}

impl Default for ProviderModelConfig {
    fn default() -> Self {
        Self {
            model: String::new(),
            enabled: true,
            request: ModelRequestConfig::default(),
        }
    }
}

impl ProviderConfig {
    fn normalize_for_key(mut self, provider_key: &str) -> Self {
        self.kind = self.kind.trim().to_lowercase();
        if self.kind.is_empty() {
            self.kind = provider_key.to_string();
        }

        self.kind = match self.kind.as_str() {
            "openai-standard" | "openai_standard" => "openai".to_string(),
            "openai-codex" | "openai_codex" => "codex".to_string(),
            other => other.to_string(),
        };

        self.api_endpoint = self.api_endpoint.trim().trim_end_matches('/').to_string();
        if self.api_endpoint.is_empty() {
            self.api_endpoint = "https://api.openai.com/v1".to_string();
        }
        self.models_endpoint_candidates = self
            .models_endpoint_candidates
            .iter()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .collect();

        self.stream_transport = self.stream_transport.trim().to_lowercase();
        if self.stream_transport != "websocket" && self.stream_transport != "sse" {
            self.stream_transport = "websocket".to_string();
        }

        self.realtime_endpoint = self
            .realtime_endpoint
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.trim_end_matches('/').to_string());

        self.credential_env = self.credential_env.trim().to_string();
        if self.credential_env.is_empty() {
            self.credential_env = format!(
                "{}_API_KEY",
                provider_key
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
                    .collect::<String>()
                    .to_uppercase()
            );
        }

        if matches!(self.request_timeout_seconds, Some(0)) {
            self.request_timeout_seconds = None;
        }

        let mut normalized_models = BTreeMap::new();
        for (raw_key, mut model_cfg) in std::mem::take(&mut self.models) {
            let model_key = raw_key.trim().to_string();
            if model_key.is_empty() {
                continue;
            }
            model_cfg.model = model_cfg.model.trim().to_string();
            if model_cfg.model.is_empty() {
                model_cfg.model = model_key.clone();
            }
            model_cfg.request.normalize();
            normalized_models.insert(model_key, model_cfg);
        }
        self.models = normalized_models;

        self
    }

    pub fn resolved_credential(&self) -> Option<String> {
        let from_config = self
            .credential
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);

        from_config.or_else(|| {
            std::env::var(&self.credential_env)
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
        })
    }

    pub fn resolved_realtime_endpoint(&self) -> String {
        if let Some(url) = &self.realtime_endpoint {
            return url.clone();
        }

        if self.api_endpoint.starts_with("https://") {
            return format!("wss://{}", self.api_endpoint.trim_start_matches("https://"));
        }
        if self.api_endpoint.starts_with("http://") {
            return format!("ws://{}", self.api_endpoint.trim_start_matches("http://"));
        }

        format!("wss://{}", self.api_endpoint)
    }

    pub fn resolved_timeout_seconds(&self, global_default: u64) -> u64 {
        self.request_timeout_seconds.unwrap_or(global_default)
    }
}

impl ModelRequestConfig {
    fn normalize(&mut self) {
        if let Some(value) = self.reasoning_effort.as_deref() {
            let trimmed = value.trim().to_lowercase();
            self.reasoning_effort = if trimmed.is_empty() {
                None
            } else {
                Some(trimmed)
            };
        }
    }
}

impl McpRuntimeConfig {
    fn normalize(mut self) -> Self {
        self.stdio.env = normalize_string_map(std::mem::take(&mut self.stdio.env));
        self
    }
}

impl McpServerConfig {
    fn normalize(mut self) -> Self {
        self.command = self.command.trim().to_string();
        self.args = self
            .args
            .iter()
            .map(|arg| arg.trim().to_string())
            .filter(|arg| !arg.is_empty())
            .collect();
        self.env = normalize_string_map(std::mem::take(&mut self.env));
        self.headers = normalize_string_map(std::mem::take(&mut self.headers));
        self.cwd = self
            .cwd
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        self.uri = self
            .uri
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        self.auth_header = self
            .auth_header
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned);
        if matches!(self.channel_buffer_capacity, Some(0)) {
            self.channel_buffer_capacity = None;
        }

        self
    }
}

fn normalize_string_map(map: BTreeMap<String, String>) -> BTreeMap<String, String> {
    let mut normalized = BTreeMap::new();
    for (raw_key, raw_value) in map {
        let key = raw_key.trim().to_string();
        if key.is_empty() {
            continue;
        }
        normalized.insert(key, raw_value.trim().to_string());
    }
    normalized
}

fn parse_model_ref(model_ref: &str) -> Option<(String, String)> {
    let trimmed = model_ref.trim();
    let (provider, model_key) = trimmed.split_once('/')?;
    let provider = provider.trim().to_lowercase();
    let model_key = model_key.trim().to_string();
    if provider.is_empty() || model_key.is_empty() {
        return None;
    }
    Some((provider, model_key))
}

fn model_ref(provider_key: &str, model_key: &str) -> String {
    format!("{}/{}", provider_key, model_key)
}

impl AppConfig {
    pub fn normalize(mut self) -> Self {
        self.active_provider = self.active_provider.trim().to_lowercase();
        self.system_prompt = self.system_prompt.trim().to_string();
        if self.system_prompt.is_empty() {
            self.system_prompt = DEFAULT_SYSTEM_PROMPT.to_string();
        }

        if self.request_timeout_seconds == 0 {
            self.request_timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
        }

        let mut normalized_providers = BTreeMap::new();
        for (raw_key, provider) in std::mem::take(&mut self.providers) {
            let provider_key = raw_key.trim().to_lowercase();
            if provider_key.is_empty() {
                continue;
            }
            normalized_providers.insert(
                provider_key.clone(),
                provider.normalize_for_key(&provider_key),
            );
        }

        if normalized_providers.is_empty() {
            normalized_providers.insert("openai".to_string(), ProviderConfig::default());
        }
        self.providers = normalized_providers;

        if self.active_provider.is_empty() || !self.providers.contains_key(&self.active_provider) {
            self.active_provider = self
                .providers
                .first_key_value()
                .map(|(k, _)| k.clone())
                .unwrap_or_else(|| "openai".to_string());
        }

        let has_any_model = self
            .providers
            .values()
            .any(|provider| provider.models.values().any(|model| model.enabled));
        if !has_any_model {
            if let Some(provider) = self.providers.get_mut(&self.active_provider) {
                provider.models.insert(
                    "gpt-5-mini".to_string(),
                    ProviderModelConfig {
                        model: "gpt-5-mini".to_string(),
                        enabled: true,
                        request: ModelRequestConfig::default(),
                    },
                );
            }
        }

        self.default_model = self.default_model.trim().to_string();
        if parse_model_ref(&self.default_model).is_none() {
            self.default_model = DEFAULT_DEFAULT_MODEL_REF.to_string();
        }
        if self.resolve_model_ref(&self.default_model).is_none() {
            self.default_model = self
                .configured_models()
                .into_iter()
                .next()
                .unwrap_or_else(|| DEFAULT_DEFAULT_MODEL_REF.to_string());
        }

        self.utility_small_model = self.utility_small_model.trim().to_string();
        if parse_model_ref(&self.utility_small_model).is_none()
            || self.resolve_model_ref(&self.utility_small_model).is_none()
        {
            self.utility_small_model = self.default_model.clone();
        }

        self.mcp_runtime = self.mcp_runtime.normalize();

        let mut normalized_mcp_servers = BTreeMap::new();
        for (raw_key, mcp_server) in std::mem::take(&mut self.mcp_servers) {
            let server_key = raw_key.trim().to_lowercase();
            if server_key.is_empty() {
                continue;
            }
            let server = mcp_server.normalize();
            normalized_mcp_servers.insert(server_key, server);
        }
        self.mcp_servers = normalized_mcp_servers;

        self
    }

    pub fn configured_models(&self) -> Vec<String> {
        let mut models = Vec::new();
        for (provider_key, provider) in &self.providers {
            for (model_key, model) in &provider.models {
                if model.enabled {
                    models.push(model_ref(provider_key, model_key));
                }
            }
        }
        models.sort();
        models
    }

    fn resolve_model_ref(
        &self,
        full_ref: &str,
    ) -> Option<(String, ProviderConfig, String, ProviderModelConfig)> {
        let (provider_key, model_key) = parse_model_ref(full_ref)?;
        let provider = self.providers.get(&provider_key)?.clone();
        let model_cfg = provider.models.get(&model_key)?.clone();
        if !model_cfg.enabled {
            return None;
        }
        Some((provider_key, provider, model_key, model_cfg))
    }

    pub fn resolve_model_profile(
        &self,
        requested: Option<&str>,
    ) -> Result<ResolvedModelConfig, String> {
        let requested_ref = requested.map(str::trim).filter(|s| !s.is_empty());
        let chosen_ref = requested_ref.unwrap_or(&self.default_model).to_string();

        let resolved = self
            .resolve_model_ref(&chosen_ref)
            .or_else(|| self.resolve_model_ref(&self.default_model))
            .ok_or_else(|| {
                format!(
                    "Model '{}' not found or disabled. Expected format: {{provider}}/{{model_id}}",
                    chosen_ref
                )
            })?;

        let (provider_key, provider, model_key, model_cfg) = resolved;

        let resolved_ref = model_ref(&provider_key, &model_key);
        Ok(ResolvedModelConfig {
            profile_key: resolved_ref.clone(),
            provider_key,
            provider: provider.clone(),
            model_id: model_cfg.model.clone(),
            model_ref: resolved_ref,
            system_prompt: self.system_prompt.clone(),
            request: model_cfg.request.clone(),
            timeout_seconds: provider.resolved_timeout_seconds(self.request_timeout_seconds),
        })
    }

    pub fn utility_small_model_key(&self) -> &str {
        &self.utility_small_model
    }

    pub fn provider_keys(&self) -> Vec<String> {
        let mut keys = self.providers.keys().cloned().collect::<Vec<_>>();
        keys.sort();
        keys
    }

    pub fn resolved_provider(&self, provider_key: &str) -> Result<ProviderConfig, String> {
        let key = provider_key.trim().to_lowercase();
        self.providers
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("Provider '{}' not found in config", provider_key))
    }
}

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
    let normalized = parsed.normalize();

    let _ = persist_config_if_changed(&path, &raw, &normalized)?;

    Ok(normalized)
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
        credential_env: active_provider_config.credential_env,
        request_timeout_seconds: config.request_timeout_seconds,
    })
}

fn default_config_yaml() -> String {
    [
        "# AgentJax configuration",
        "# Home directory: AGENTJAX_HOME (default: ~/.agentjax)",
        "# Config path: $AGENTJAX_HOME/config.yaml",
        "",
        "active_provider: \"openai\"",
        "default_model: \"openai/gpt-5-mini\"",
        "utility_small_model: \"openai/gpt-5-mini\"",
        "request_timeout_seconds: 120",
        "system_prompt: \"You are Codex, a helpful AI assistant. Follow the user's instructions.\"",
        "",
        "providers:",
        "  openai:",
        "    kind: \"openai\"",
        "    api_endpoint: \"https://api.openai.com/v1\"",
        "    realtime_endpoint: \"\"",
        "    stream_transport: \"websocket\"",
        "    credential: \"\"",
        "    credential_env: \"OPENAI_API_KEY\"",
        "    request_timeout_seconds: 120",
        "    models:",
        "      gpt-5-mini:",
        "        model: \"gpt-5-mini\"",
        "        enabled: true",
        "        request:",
        "          temperature: null",
        "          top_p: null",
        "          top_k: null",
        "          max_output_tokens: null",
        "          frequency_penalty: null",
        "          presence_penalty: null",
        "          reasoning_effort: null",
        "          extra_body: {}",
        "      gpt-5:",
        "        model: \"gpt-5\"",
        "        enabled: true",
        "        request:",
        "          temperature: null",
        "          top_p: null",
        "          top_k: null",
        "          max_output_tokens: null",
        "          frequency_penalty: null",
        "          presence_penalty: null",
        "          reasoning_effort: null",
        "          extra_body: {}",
        "",
        "mcp_runtime:",
        "  stdio:",
        "    inherit_parent_env: false",
        "    env: {}",
        "",
        "mcp_servers: {}",
        "",
    ]
    .join("\n")
}

fn default_true() -> bool {
    true
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_model_with_provider_scoped_reference() {
        let cfg = AppConfig::default().normalize();
        let resolved = cfg
            .resolve_model_profile(Some("openai/gpt-5"))
            .expect("resolve model");
        assert_eq!(resolved.provider_key, "openai");
        assert_eq!(resolved.model_id, "gpt-5");
        assert_eq!(resolved.model_ref, "openai/gpt-5");
    }

    #[test]
    fn falls_back_to_default_when_requested_model_invalid() {
        let cfg = AppConfig::default().normalize();
        let resolved = cfg
            .resolve_model_profile(Some("openai/not-exist"))
            .expect("fallback to default");
        assert_eq!(resolved.model_ref, cfg.default_model);
    }
}
