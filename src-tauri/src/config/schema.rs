use crate::config::constants::{
    DEFAULT_DEFAULT_MODEL_REF, DEFAULT_TIMEOUT_SECONDS, DEFAULT_UTILITY_SMALL_MODEL_REF,
    default_mcp_startup_timeout_ms, default_mcp_tool_timeout_ms, default_true,
};
use crate::config::prompt_composer::{CompiledPromptAssembly, PromptComposerConfig};
use crate::providers::registry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

fn default_language() -> String {
    "auto".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub active_provider: String,
    pub providers: BTreeMap<String, ProviderConfig>,
    pub default_model: String,
    pub utility_small_model: String,
    #[serde(default)]
    pub prompt_composer: PromptComposerConfig,
    pub request_timeout_seconds: u64,
    pub show_advanced_request_options: bool,
    pub enable_developer_tools: bool,
    #[serde(default)]
    pub mcp_runtime: McpRuntimeConfig,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub tool_manager: ToolManagerConfig,
    #[serde(default = "default_language")]
    pub language: String,
}

/// User-facing tool exposure policy.
///
/// The first read-only Tools Manager surface uses this to report effective
/// availability. Later management actions can patch the same structure without
/// changing provider execution paths or source-specific config models.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolManagerConfig {
    #[serde(default)]
    pub native_tools: BTreeMap<String, ToolEnabledConfig>,
    #[serde(default)]
    pub plugin_tools: BTreeMap<String, ToolSourcePolicyConfig>,
    #[serde(default)]
    pub mcp_tools: BTreeMap<String, McpToolSourcePolicyConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolEnabledConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolSourcePolicyConfig {
    pub enabled: bool,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolEnabledConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpToolSourcePolicyConfig {
    pub enabled: bool,
    pub exposure: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolEnabledConfig>,
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
    #[serde(default)]
    pub unfolded: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpRuntimeConfig {
    pub stdio: McpStdioRuntimeConfig,
    pub startup_timeout_ms: u64,
    pub tool_timeout_ms: u64,
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
    #[serde(default)]
    pub query_params: BTreeMap<String, String>,
    #[serde(default)]
    pub http_headers: BTreeMap<String, String>,
    #[serde(default)]
    pub env_http_headers: BTreeMap<String, String>,
    pub realtime_endpoint: Option<String>,
    #[serde(default = "default_true")]
    pub supports_websockets: bool,
    pub stream_transport: String,
    pub credential: Option<String>,
    pub credential_env: String,
    pub request_timeout_seconds: Option<u64>,
    pub request_max_retries: Option<u32>,
    pub stream_max_retries: Option<u32>,
    pub stream_idle_timeout_ms: Option<u64>,
    pub websocket_connect_timeout_ms: Option<u64>,
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
    pub profile_key: String,
    pub provider_key: String,
    pub provider: ProviderConfig,
    pub model_id: String,
    pub model_ref: String,
    pub system_prompt: String,
    pub prompt_assembly: CompiledPromptAssembly,
    pub request: ModelRequestConfig,
    pub timeout_seconds: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        let mut providers = BTreeMap::new();
        let default_provider = registry::default_provider_definition();
        providers.insert(default_provider.kind.to_string(), ProviderConfig::default());

        Self {
            active_provider: default_provider.kind.to_string(),
            providers,
            default_model: DEFAULT_DEFAULT_MODEL_REF.to_string(),
            utility_small_model: DEFAULT_UTILITY_SMALL_MODEL_REF.to_string(),
            prompt_composer: PromptComposerConfig::default(),
            request_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            show_advanced_request_options: false,
            enable_developer_tools: false,
            mcp_runtime: McpRuntimeConfig::default(),
            mcp_servers: BTreeMap::new(),
            tool_manager: ToolManagerConfig::default(),
            language: default_language(),
        }
    }
}

impl Default for ToolEnabledConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

impl Default for ToolSourcePolicyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            tools: BTreeMap::new(),
        }
    }
}

impl Default for McpToolSourcePolicyConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            exposure: None,
            tools: BTreeMap::new(),
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
            unfolded: false,
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

impl Default for McpRuntimeConfig {
    fn default() -> Self {
        Self {
            stdio: McpStdioRuntimeConfig::default(),
            startup_timeout_ms: default_mcp_startup_timeout_ms(),
            tool_timeout_ms: default_mcp_tool_timeout_ms(),
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        registry::default_provider_config()
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
