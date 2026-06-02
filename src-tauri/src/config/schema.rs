use crate::config::constants::{
    DEFAULT_DEFAULT_MODEL_REF, DEFAULT_TIMEOUT_SECONDS, DEFAULT_UTILITY_SMALL_MODEL_REF,
    default_mcp_startup_timeout_ms, default_mcp_tool_timeout_ms, default_true,
};
use crate::config::prompt_composer::{CompiledPromptAssembly, PromptComposerConfig};
use crate::provider_api::registry;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

fn default_language() -> String {
    "auto".to_string()
}

// ── Sub-Agent Config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SubAgentConfig {
    /// Maximum concurrent sub-agents allowed process-wide.
    pub max_concurrent: usize,
    /// Default maximum turns for a sub-agent.
    pub default_max_turns: usize,
    /// Hard cap on sub-agent turns.
    pub hard_max_turns: usize,
    /// Maximum time a sub-agent may run before being timed out (seconds).
    pub timeout_secs: u64,
    /// Whether git worktree isolation is enabled.
    pub worktree_enabled: bool,
}

impl Default for SubAgentConfig {
    fn default() -> Self {
        Self {
            max_concurrent: 4,
            default_max_turns: 5,
            hard_max_turns: 10,
            timeout_secs: 300,
            worktree_enabled: false,
        }
    }
}

// ── Memory Config ─────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MemoryConfig {
    /// Whether the memory system is enabled.
    pub enabled: bool,
    /// Maximum tokens for the MEMORY.md index injected into context.
    pub max_index_tokens: u32,
    /// Whether to auto-inject the memory index into each conversation turn.
    pub auto_inject: bool,
    /// Directory where memory files are stored (relative to agentjax home).
    pub storage_dir: String,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_index_tokens: 2000,
            auto_inject: true,
            storage_dir: "memory".to_string(),
        }
    }
}

// ── Street Config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StreetConfig {
    /// Whether the Street notification system is enabled.
    pub enabled: bool,
    /// Minimum priority to auto-trigger a new turn ("never", "urgent", "high", "normal", "low").
    pub auto_trigger_priority: String,
    /// Maximum Street items retained per conversation.
    pub max_items_per_conversation: usize,
}

impl Default for StreetConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_trigger_priority: "urgent".to_string(),
            max_items_per_conversation: 100,
        }
    }
}

// ── Conversation Config ────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ConversationConfig {
    /// Whether to write a JSONL backup alongside the LCM SQLite store.
    /// JSONL is a plain-text fallback — more resilient to corruption than
    /// binary SQLite, but adds I/O overhead. Disable for better performance
    /// when LCM is the sole source of truth.
    pub jsonl_backup_enabled: bool,
}

impl Default for ConversationConfig {
    fn default() -> Self {
        Self {
            jsonl_backup_enabled: true,
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    #[serde(default = "default_language")]
    pub language: String,
    pub active_provider: String,
    pub default_model: String,
    pub utility_small_model: String,
    pub request_timeout_seconds: u64,
    pub show_advanced_request_options: bool,
    pub enable_developer_tools: bool,
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub prompt_composer: PromptComposerConfig,
    #[serde(default)]
    pub lcm: crate::lcm::LcmConfig,
    #[serde(default)]
    pub sub_agent: SubAgentConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub street: StreetConfig,
    #[serde(default)]
    pub conversation: ConversationConfig,
    #[serde(default)]
    pub mcp_runtime: McpRuntimeConfig,
    #[serde(default)]
    pub mcp_servers: BTreeMap<String, McpServerConfig>,
    #[serde(default)]
    pub tool_manager: ToolManagerConfig,
    #[serde(default)]
    pub plugin_manager: PluginManagerConfig,
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
    #[serde(default)]
    pub context_tools: BTreeMap<String, ToolEnabledConfig>,
}

/// Plugin lifecycle and permission configuration.
///
/// Controls whether a plugin is enabled at the plugin-manager level and
/// allows the user to override individual sandbox permissions declared in the
/// plugin's manifest.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct PluginManagerConfig {
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginEntryConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginEntryConfig {
    /// Whether the plugin is enabled at the plugin-manager level.
    /// A disabled plugin cannot register tools or providers.
    pub enabled: bool,
    /// Optional per-plugin permission overrides.
    /// When `None`, the plugin's manifest-declared sandbox policy is used.
    /// When `Some`, these values override the corresponding manifest fields.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub permissions: Option<PluginPermissionOverride>,
}

impl Default for PluginEntryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            permissions: None,
        }
    }
}

/// Per-plugin sandbox permission overrides that users can toggle individually.
///
/// Each field is `Option<bool>` so we can distinguish between "user has not
/// set an override" (`None`) and "user explicitly set to false" (`Some(false)`).
/// When `None`, the effective permission falls back to the manifest-declared value.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginPermissionOverride {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_network: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_file_read: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_file_write: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_process_spawn: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub allow_env_read: Option<bool>,
}

impl Default for PluginPermissionOverride {
    fn default() -> Self {
        Self {
            allow_network: None,
            allow_file_read: None,
            allow_file_write: None,
            allow_process_spawn: None,
            allow_env_read: None,
        }
    }
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
    pub enabled: bool,
    pub transport: McpTransportKind,
    pub command: String,
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub cwd: Option<String>,
    pub uri: Option<String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub auth_header: Option<String>,
    #[serde(default = "default_true")]
    pub use_global_stdio_env: bool,
    pub inherit_parent_env: Option<bool>,
    #[serde(default = "default_true")]
    pub allow_stateless: bool,
    pub channel_buffer_capacity: Option<usize>,
    #[serde(default = "default_true")]
    pub reinit_on_expired_session: bool,
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
    #[serde(default)]
    pub models: BTreeMap<String, ProviderModelConfig>,
    #[serde(flatten)]
    pub custom_settings: serde_json::Map<String, Value>,
}

impl ProviderConfig {
    pub fn api_endpoint(&self) -> String {
        self.custom_settings
            .get("apiEndpoint")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    #[allow(dead_code)] // Reserved for future use
    pub fn models_endpoint_candidates(&self) -> Vec<String> {
        self.custom_settings
            .get("modelsEndpointCandidates")
            .and_then(Value::as_array)
            .map(|arr| {
                arr.iter()
                    .filter_map(|val| val.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default()
    }

    #[allow(dead_code)] // Reserved for future use
    pub fn query_params(&self) -> BTreeMap<String, String> {
        self.custom_settings
            .get("queryParams")
            .and_then(Value::as_object)
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn http_headers(&self) -> BTreeMap<String, String> {
        self.custom_settings
            .get("httpHeaders")
            .and_then(Value::as_object)
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn env_http_headers(&self) -> BTreeMap<String, String> {
        self.custom_settings
            .get("envHttpHeaders")
            .and_then(Value::as_object)
            .map(|obj| {
                obj.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn realtime_endpoint(&self) -> Option<String> {
        self.custom_settings
            .get("realtimeEndpoint")
            .and_then(|val| {
                if val.is_null() {
                    None
                } else {
                    val.as_str().map(String::from)
                }
            })
    }

    pub fn supports_websockets(&self) -> bool {
        self.custom_settings
            .get("supportsWebsockets")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    }

    pub fn stream_transport(&self) -> String {
        self.custom_settings
            .get("streamTransport")
            .and_then(Value::as_str)
            .unwrap_or("sse")
            .to_string()
    }

    pub fn credential(&self) -> Option<String> {
        self.custom_settings.get("credential").and_then(|val| {
            if val.is_null() {
                None
            } else {
                val.as_str().map(String::from)
            }
        })
    }

    pub fn credential_env(&self) -> String {
        self.custom_settings
            .get("credentialEnv")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    }

    pub fn request_timeout_seconds(&self) -> Option<u64> {
        self.custom_settings
            .get("requestTimeoutSeconds")
            .and_then(Value::as_u64)
    }

    #[allow(dead_code)] // Reserved for future use
    pub fn request_max_retries(&self) -> Option<u32> {
        self.custom_settings
            .get("requestMaxRetries")
            .and_then(Value::as_u64)
            .map(|v| v as u32)
    }

    #[allow(dead_code)] // Reserved for future use
    pub fn stream_max_retries(&self) -> Option<u32> {
        self.custom_settings
            .get("streamMaxRetries")
            .and_then(Value::as_u64)
            .map(|v| v as u32)
    }

    #[allow(dead_code)] // Reserved for future use
    pub fn stream_idle_timeout_ms(&self) -> Option<u64> {
        self.custom_settings
            .get("streamIdleTimeoutMs")
            .and_then(Value::as_u64)
    }

    #[allow(dead_code)] // Reserved for future use
    pub fn websocket_connect_timeout_ms(&self) -> Option<u64> {
        self.custom_settings
            .get("websocketConnectTimeoutMs")
            .and_then(Value::as_u64)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderModelConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional user-facing friendly name.
    /// When absent, the model ID (map key) is shown instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
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
        providers.insert(
            default_provider.kind.to_string(),
            registry::default_provider_config(),
        );

        Self {
            language: default_language(),
            active_provider: default_provider.kind.to_string(),
            default_model: DEFAULT_DEFAULT_MODEL_REF.to_string(),
            utility_small_model: DEFAULT_UTILITY_SMALL_MODEL_REF.to_string(),
            request_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            show_advanced_request_options: false,
            enable_developer_tools: false,
            providers,
            prompt_composer: PromptComposerConfig::default(),
            lcm: crate::lcm::LcmConfig::default(),
            sub_agent: SubAgentConfig::default(),
            memory: MemoryConfig::default(),
            street: StreetConfig::default(),
            conversation: ConversationConfig::default(),
            mcp_runtime: McpRuntimeConfig::default(),
            mcp_servers: BTreeMap::new(),
            tool_manager: ToolManagerConfig::default(),
            plugin_manager: PluginManagerConfig::default(),
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
            enabled: true,
            transport: McpTransportKind::Stdio,
            command: String::new(),
            args: Vec::new(),
            env: BTreeMap::new(),
            cwd: None,
            uri: None,
            headers: BTreeMap::new(),
            auth_header: None,
            use_global_stdio_env: true,
            inherit_parent_env: None,
            allow_stateless: true,
            channel_buffer_capacity: None,
            reinit_on_expired_session: true,
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
        Self {
            kind: String::new(),
            models: BTreeMap::new(),
            custom_settings: serde_json::Map::new(),
        }
    }
}

impl Default for ProviderModelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            name: None,
            request: ModelRequestConfig::default(),
        }
    }
}
