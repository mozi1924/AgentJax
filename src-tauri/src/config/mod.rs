use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

const APP_CONFIG_DIR_NAME: &str = "AgentJax";
const CONFIG_FILE_NAME: &str = "config.yaml";
const DEFAULT_SYSTEM_PROMPT: &str =
    "You are Codex, a helpful AI assistant. Follow the user's instructions.";
const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const DEFAULT_UTILITY_SMALL_MODEL: &str = "gpt-5-mini";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub active_provider: String,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub default_model: String,
    pub utility_small_model: String,
    pub model_profiles: BTreeMap<String, ModelProfile>,
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
    pub store_responses: bool,
    pub system_prompt: String,
    pub request_timeout_seconds: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelProfile {
    pub provider: String,
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
    pub profile_key: String,
    pub provider_key: String,
    pub provider: ProviderConfig,
    pub model_id: String,
    pub request: ModelRequestConfig,
    pub timeout_seconds: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        providers.insert("openai".to_string(), ProviderConfig::default());

        let mut model_profiles = BTreeMap::new();
        model_profiles.insert(
            "gpt-5-mini".to_string(),
            ModelProfile {
                provider: "openai".to_string(),
                model: "gpt-5-mini".to_string(),
                enabled: true,
                request: ModelRequestConfig::default(),
            },
        );
        model_profiles.insert(
            "gpt-5".to_string(),
            ModelProfile {
                provider: "openai".to_string(),
                model: "gpt-5".to_string(),
                enabled: true,
                request: ModelRequestConfig::default(),
            },
        );

        Self {
            active_provider: "openai".to_string(),
            providers,
            default_model: "gpt-5-mini".to_string(),
            utility_small_model: DEFAULT_UTILITY_SMALL_MODEL.to_string(),
            model_profiles,
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
        Self {
            kind: "openai".to_string(),
            api_endpoint: "https://api.openai.com/v1".to_string(),
            models_endpoint_candidates: Vec::new(),
            realtime_endpoint: None,
            stream_transport: "websocket".to_string(),
            credential: None,
            credential_env: "OPENAI_API_KEY".to_string(),
            store_responses: false,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_string(),
            request_timeout_seconds: None,
        }
    }
}

impl Default for ModelProfile {
    fn default() -> Self {
        Self {
            provider: "".to_string(),
            model: "".to_string(),
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

        self.system_prompt = self.system_prompt.trim().to_string();
        if self.system_prompt.is_empty() {
            self.system_prompt = DEFAULT_SYSTEM_PROMPT.to_string();
        }

        if matches!(self.request_timeout_seconds, Some(0)) {
            self.request_timeout_seconds = None;
        }

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

impl ModelProfile {
    fn normalize(mut self, default_provider: &str) -> Self {
        self.provider = self.provider.trim().to_lowercase();
        if self.provider.is_empty() {
            self.provider = default_provider.to_string();
        }

        self.model = self.model.trim().to_string();
        self.request.normalize();

        self
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

impl AppConfig {
    pub fn normalize(mut self) -> Self {
        self.active_provider = self.active_provider.trim().to_lowercase();
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

        let mut normalized_profiles = BTreeMap::new();
        for (raw_key, profile) in std::mem::take(&mut self.model_profiles) {
            let profile_key = raw_key.trim().to_string();
            if profile_key.is_empty() {
                continue;
            }

            let profile = profile.normalize(&self.active_provider);
            if profile.model.is_empty() {
                continue;
            }
            if !self.providers.contains_key(&profile.provider) {
                continue;
            }

            normalized_profiles.insert(profile_key, profile);
        }

        if normalized_profiles.is_empty() {
            normalized_profiles.insert(
                "gpt-5-mini".to_string(),
                ModelProfile {
                    provider: self.active_provider.clone(),
                    model: "gpt-5-mini".to_string(),
                    enabled: true,
                    request: ModelRequestConfig::default(),
                },
            );
            normalized_profiles.insert(
                "gpt-5".to_string(),
                ModelProfile {
                    provider: self.active_provider.clone(),
                    model: "gpt-5".to_string(),
                    enabled: true,
                    request: ModelRequestConfig::default(),
                },
            );
        }
        self.model_profiles = normalized_profiles;

        self.default_model = self.default_model.trim().to_string();
        let default_is_usable = self
            .model_profiles
            .get(&self.default_model)
            .map(|p| p.enabled)
            .unwrap_or(false);

        if !default_is_usable {
            if let Some((profile_key, _)) = self.model_profiles.iter().find(|(_, p)| p.enabled) {
                self.default_model = profile_key.clone();
            } else if let Some((profile_key, _)) = self.model_profiles.first_key_value() {
                self.default_model = profile_key.clone();
            }
        }

        if self
            .model_profiles
            .get(&self.default_model)
            .map(|p| !p.enabled)
            .unwrap_or(false)
        {
            if let Some(profile) = self.model_profiles.get_mut(&self.default_model) {
                profile.enabled = true;
            }
        }

        self.utility_small_model = self.utility_small_model.trim().to_string();
        let utility_is_usable = self
            .model_profiles
            .get(&self.utility_small_model)
            .map(|p| p.enabled)
            .unwrap_or(false);

        if !utility_is_usable {
            self.utility_small_model = self.default_model.clone();
        }

        if self
            .model_profiles
            .get(&self.utility_small_model)
            .map(|p| !p.enabled)
            .unwrap_or(false)
        {
            if let Some(profile) = self.model_profiles.get_mut(&self.utility_small_model) {
                profile.enabled = true;
            }
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
        self.model_profiles
            .iter()
            .filter(|(_, profile)| profile.enabled)
            .map(|(profile_key, _)| profile_key.clone())
            .collect()
    }

    pub fn resolve_model_profile(
        &self,
        requested: Option<&str>,
    ) -> Result<ResolvedModelConfig, String> {
        let requested_key = requested.map(str::trim).filter(|s| !s.is_empty());

        let profile_key = if let Some(key) = requested_key {
            if self.model_profiles.contains_key(key) {
                key.to_string()
            } else {
                self.default_model.clone()
            }
        } else {
            self.default_model.clone()
        };

        let profile = self
            .model_profiles
            .get(&profile_key)
            .ok_or_else(|| format!("Model profile '{}' not found", profile_key))?;

        if !profile.enabled {
            return Err(format!("Model profile '{}' is disabled", profile_key));
        }

        let provider_key = profile.provider.clone();
        let provider = self
            .providers
            .get(&provider_key)
            .ok_or_else(|| {
                format!(
                    "Provider '{}' referenced by model profile '{}' is missing",
                    provider_key, profile_key
                )
            })?
            .clone();

        Ok(ResolvedModelConfig {
            profile_key,
            provider_key,
            model_id: profile.model.clone(),
            request: profile.request.clone(),
            timeout_seconds: provider.resolved_timeout_seconds(self.request_timeout_seconds),
            provider,
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
    let base =
        dirs::config_dir().ok_or_else(|| "Unable to locate OS config directory".to_string())?;
    Ok(base.join(APP_CONFIG_DIR_NAME))
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
        "# macOS: ~/Library/Application Support/AgentJax/config.yaml",
        "# Linux: ~/.config/AgentJax/config.yaml",
        "# Windows: %APPDATA%\\AgentJax\\config.yaml",
        "",
        "active_provider: \"openai\"",
        "request_timeout_seconds: 120",
        "",
        "providers:",
        "  openai:",
        "    kind: \"openai\"",
        "    api_endpoint: \"https://api.openai.com/v1\"",
        "    realtime_endpoint: \"\"",
        "    stream_transport: \"websocket\"",
        "    credential: \"\"",
        "    credential_env: \"OPENAI_API_KEY\"",
        "    store_responses: false",
        "    system_prompt: \"You are Codex, a helpful AI assistant. Follow the user's instructions.\"",
        "    request_timeout_seconds: 120",
        "",
        "model_profiles:",
        "  gpt-5-mini:",
        "    provider: \"openai\"",
        "    model: \"gpt-5-mini\"",
        "    enabled: true",
        "    request:",
        "      temperature: null",
        "      top_p: null",
        "      top_k: null",
        "      max_output_tokens: null",
        "      frequency_penalty: null",
        "      presence_penalty: null",
        "      reasoning_effort: null",
        "      extra_body: {}",
        "  gpt-5:",
        "    provider: \"openai\"",
        "    model: \"gpt-5\"",
        "    enabled: true",
        "    request:",
        "      temperature: null",
        "      top_p: null",
        "      top_k: null",
        "      max_output_tokens: null",
        "      frequency_penalty: null",
        "      presence_penalty: null",
        "      reasoning_effort: null",
        "      extra_body: {}",
        "",
        "default_model: \"gpt-5-mini\"",
        "utility_small_model: \"gpt-5-mini\"",
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
