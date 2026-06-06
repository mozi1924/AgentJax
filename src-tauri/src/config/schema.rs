use crate::config::constants::{
    default_mcp_startup_timeout_ms, default_mcp_tool_timeout_ms, default_true,
};
use crate::config::prompt_composer::CompiledPromptAssembly;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};

fn default_language() -> String {
    "auto".to_string()
}

fn default_active_agent_id() -> String {
    crate::config::constants::DEFAULT_AGENT_ID.to_string()
}

// ── Sub-Agent Config ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

// ── Context Management Config ─────────────────────────────────────────────

/// Combined configuration for the Context Management subsystem.
/// Merges the former LCM (Lossless Context Management) settings, Street
/// notification config, and conversation JSONL backup toggle into a single
/// top-level `context_management` section.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ContextManagementConfig {
    // ── LCM fields (flattened at this level) ─────────────────────────────
    /// When true, the soft/hard/large-file thresholds are computed dynamically
    /// from the active model's context window.
    pub dynamic_thresholds: bool,
    /// Soft token threshold for async compaction.
    pub soft_token_threshold: u32,
    /// Hard token threshold for blocking compaction.
    pub hard_token_threshold: u32,
    /// Large file token threshold.
    pub large_file_token_threshold: u32,
    /// Compaction timeout (seconds).
    pub compaction_timeout_secs: u32,
    /// Maximum messages in a single compact block.
    pub max_compact_block_size: usize,
    /// Maximum DAG summary depth.
    pub max_summary_depth: u32,
    /// Truncation max tokens.
    pub truncation_max_tokens: u32,
    /// Grep page size.
    pub grep_page_size: usize,
    /// Summarization model reference.
    #[serde(default)]
    pub summarization_model: String,
    /// Tokenizer model ID for accurate token counting.
    #[serde(default)]
    pub tokenizer_model_id: Option<String>,

    // ── Street ──────────────────────────────────────────────────────────
    /// Whether the Street notification system is enabled.
    pub street_enabled: bool,
    /// Minimum priority to auto-trigger a new turn.
    pub street_auto_trigger_priority: String,
    /// Maximum Street items retained per conversation.
    pub street_max_items_per_conversation: usize,
    /// Maximum time (seconds) to wait for sub-agents / background jobs
    /// before giving up and ending the turn. Prevents the conversation from
    /// hanging indefinitely when a sub-agent silently fails. Default: 300 (5 min).
    pub street_resume_timeout_secs: u64,

    // ── JSONL backup ────────────────────────────────────────────────────
    /// Whether to write a JSONL backup alongside the context store.
    pub jsonl_backup_enabled: bool,
}

impl Default for ContextManagementConfig {
    fn default() -> Self {
        Self {
            dynamic_thresholds: true,
            soft_token_threshold: 65536,
            hard_token_threshold: 131072,
            large_file_token_threshold: 25600,
            compaction_timeout_secs: 25,
            max_compact_block_size: 20,
            max_summary_depth: 5,
            truncation_max_tokens: 128,
            grep_page_size: 20,
            summarization_model: String::new(),
            tokenizer_model_id: None,
            street_enabled: true,
            street_auto_trigger_priority: "urgent".to_string(),
            street_max_items_per_conversation: 100,
            street_resume_timeout_secs: 300,
            jsonl_backup_enabled: true,
        }
    }
}

impl ContextManagementConfig {
    /// Produce effective LCM thresholds, optionally auto-computed from the model's
    /// context window when `dynamic_thresholds` is enabled.
    pub fn to_lcm_config(&self) -> crate::lcm::LcmConfig {
        crate::lcm::LcmConfig {
            dynamic_thresholds: self.dynamic_thresholds,
            soft_token_threshold: self.soft_token_threshold,
            hard_token_threshold: self.hard_token_threshold,
            large_file_token_threshold: self.large_file_token_threshold,
            compaction_timeout_secs: self.compaction_timeout_secs,
            max_compact_block_size: self.max_compact_block_size,
            max_summary_depth: self.max_summary_depth,
            truncation_max_tokens: self.truncation_max_tokens,
            grep_page_size: self.grep_page_size,
            summarization_model: self.summarization_model.clone(),
            tokenizer_model_id: self.tokenizer_model_id.clone(),
        }
    }
}

// ── AppConfig ─────────────────────────────────────────────────────────────────

// ── RAG / Embedding Config ──────────────────────────────────────────────────────

/// Configuration for the embedding provider used by RAG.
/// Credentials are resolved by referencing an existing provider config
/// via `provider_key` (e.g., "openai-responses").
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct EmbeddingProviderConfig {
    /// Embedding provider implementation name (e.g., "openai").
    pub provider: String,
    /// Optional reference to an existing provider config key for credentials.
    /// When set, the system reads apiEndpoint and credential/credentialEnv
    /// from the referenced provider config in AppConfig.providers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_key: Option<String>,
    /// Embedding model name (e.g., "text-embedding-3-small").
    pub model: String,
    /// Output vector dimensions.
    pub dimensions: usize,
}

impl Default for EmbeddingProviderConfig {
    fn default() -> Self {
        Self {
            provider: "openai".to_string(),
            provider_key: None,
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
        }
    }
}

/// RAG (Retrieval-Augmented Generation) system configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct RagConfig {
    /// Whether the RAG system is enabled.
    pub enabled: bool,
    /// Directory for vector store data (relative to agentjax home).
    pub storage_path: String,
    /// Default chunk size in characters for text splitting.
    pub chunk_size: usize,
    /// Chunk overlap in characters.
    pub chunk_overlap: usize,
    /// Default top-K results for searches.
    pub top_k: usize,
    /// Embedding provider configuration.
    #[serde(default)]
    pub embedding: EmbeddingProviderConfig,
}

impl Default for RagConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            storage_path: "rag".to_string(),
            chunk_size: 512,
            chunk_overlap: 64,
            top_k: 5,
            embedding: EmbeddingProviderConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AppConfig {
    #[serde(default = "default_language")]
    pub language: String,
    /// The currently active agent profile ID. Stored in shared config.yaml.
    #[serde(default = "default_active_agent_id")]
    pub active_agent_id: String,
    pub providers: BTreeMap<String, ProviderConfig>,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub plugin_manager: PluginManagerConfig,
}

// ── MCP Config ─────────────────────────────────────────────────────────────

/// Unified MCP configuration merging runtime settings and server definitions.
/// The outer (non-list) fields are the shared MCP runtime config; the
/// `servers` list contains individual MCP server configurations.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct McpConfig {
    pub stdio: McpStdioRuntimeConfig,
    pub startup_timeout_ms: u64,
    pub tool_timeout_ms: u64,
    pub servers: BTreeMap<String, McpServerConfig>,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            stdio: McpStdioRuntimeConfig::default(),
            startup_timeout_ms: default_mcp_startup_timeout_ms(),
            tool_timeout_ms: default_mcp_tool_timeout_ms(),
            servers: BTreeMap::new(),
        }
    }
}

impl McpConfig {
    /// Returns a reference to the runtime config portion.
    pub fn runtime(&self) -> McpRuntimeConfig {
        McpRuntimeConfig {
            stdio: self.stdio.clone(),
            startup_timeout_ms: self.startup_timeout_ms,
            tool_timeout_ms: self.tool_timeout_ms,
        }
    }
}

/// User-facing tool exposure policy.
///
/// The first read-only Tools Manager surface uses this to report effective
/// availability. Later management actions can patch the same structure without
/// changing provider execution paths or source-specific config models.
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[serde(default)]
pub struct PluginManagerConfig {
    #[serde(default)]
    pub plugins: BTreeMap<String, PluginEntryConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ToolEnabledConfig {
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ToolSourcePolicyConfig {
    pub enabled: bool,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolEnabledConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct McpToolSourcePolicyConfig {
    pub enabled: bool,
    pub exposure: Option<String>,
    #[serde(default)]
    pub tools: BTreeMap<String, ToolEnabledConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
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

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct McpRuntimeConfig {
    pub stdio: McpStdioRuntimeConfig,
    pub startup_timeout_ms: u64,
    pub tool_timeout_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
#[derive(Default)]
pub struct McpStdioRuntimeConfig {
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    pub inherit_parent_env: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum McpTransportKind {
    #[default]
    Stdio,
    StreamableHttp,
}

/// Standardized provider configuration with strongly typed fields.
///
/// Previously used `#[serde(flatten)] custom_settings` which caused:
/// - No compile-time type safety (string-keyed lookups)
/// - Duplicate keys with different naming conventions (e.g. `credential_env` vs `credentialEnv`)
/// - Empty default values cluttering the config file
///
/// The new design promotes well-known fields to first-class typed members while
/// keeping `extension_fields` for truly provider-specific settings. Serialization
/// uses camelCase to match the existing YAML convention.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default, rename_all = "camelCase")]
pub struct ProviderConfig {
    pub kind: String,
    #[serde(default)]
    pub models: BTreeMap<String, ProviderModelConfig>,

    // ── Auth ────────────────────────────────────────────────────────────
    /// Inline credential value (API key, token, etc.).
    /// If set, takes precedence over `credential_env`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
    /// Environment variable name to read the credential from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_env: Option<String>,

    // ── Network ─────────────────────────────────────────────────────────
    /// Base API endpoint URL (e.g. "https://api.deepseek.com/v1").
    #[serde(default)]
    pub api_endpoint: String,
    /// Custom HTTP headers to include in every request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub http_headers: BTreeMap<String, String>,
    /// HTTP headers sourced from environment variables (key = header name, value = env var name).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub env_http_headers: BTreeMap<String, String>,
    /// Query parameters to include in every request.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query_params: BTreeMap<String, String>,
    /// Candidates for the model listing endpoint (used for auto-discovery).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub models_endpoint_candidates: Vec<String>,
    /// Realtime endpoint URL (WebSocket).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realtime_endpoint: Option<String>,
    /// Whether WebSocket transport is supported.
    #[serde(default)]
    pub supports_websockets: bool,
    /// Stream transport: "sse" (default) or "websocket".
    #[serde(default)]
    pub stream_transport: String,

    // ── Timeouts & Retries ─────────────────────────────────────────────
    /// Request timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout_seconds: Option<u64>,
    /// Maximum retries for non-streaming requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_max_retries: Option<u32>,
    /// Maximum retries for streaming requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_max_retries: Option<u32>,
    /// Stream idle timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream_idle_timeout_ms: Option<u64>,
    /// WebSocket connect timeout in milliseconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub websocket_connect_timeout_ms: Option<u64>,

    // ── Provider-specific extension fields ──────────────────────────────
    /// Any additional provider-specific settings not covered by the standard fields above.
    /// These are flattened into the YAML/JSON output (via `#[serde(flatten)]`).
    /// Keys that overlap with the typed fields above are ignored during deserialization.
    #[serde(flatten)]
    pub extension_fields: HashMap<String, Value>,
}

impl ProviderConfig {
    // ── Convenience accessors (field wrappers for backward compat) ──────

    pub fn api_endpoint(&self) -> String {
        self.api_endpoint.clone()
    }

    pub fn credential(&self) -> Option<String> {
        self.credential.clone()
    }

    pub fn credential_env(&self) -> String {
        self.credential_env.clone().unwrap_or_default()
    }

    pub fn http_headers(&self) -> BTreeMap<String, String> {
        self.http_headers.clone()
    }

    pub fn env_http_headers(&self) -> BTreeMap<String, String> {
        self.env_http_headers.clone()
    }

    pub fn realtime_endpoint(&self) -> Option<String> {
        self.realtime_endpoint.clone()
    }

    pub fn supports_websockets(&self) -> bool {
        self.supports_websockets
    }

    pub fn stream_transport(&self) -> String {
        self.stream_transport.clone()
    }

    pub fn request_timeout_seconds(&self) -> Option<u64> {
        self.request_timeout_seconds
    }

    pub fn query_params(&self) -> BTreeMap<String, String> {
        self.query_params.clone()
    }

    pub fn models_endpoint_candidates(&self) -> Vec<String> {
        self.models_endpoint_candidates.clone()
    }

    pub fn request_max_retries(&self) -> Option<u32> {
        self.request_max_retries
    }

    pub fn stream_max_retries(&self) -> Option<u32> {
        self.stream_max_retries
    }

    pub fn stream_idle_timeout_ms(&self) -> Option<u64> {
        self.stream_idle_timeout_ms
    }

    pub fn websocket_connect_timeout_ms(&self) -> Option<u64> {
        self.websocket_connect_timeout_ms
    }

    /// Resolve the effective credential: inline value first, then environment variable.
    pub fn resolved_credential(&self) -> Option<String> {
        self.credential
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
            .or_else(|| {
                self.credential_env
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .and_then(|env_key| std::env::var(env_key).ok())
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
            })
    }

    /// Resolve the effective realtime endpoint: explicit URL, or derive from `api_endpoint`.
    pub fn resolved_realtime_endpoint(&self) -> String {
        if let Some(ref url) = self.realtime_endpoint {
            return url.clone();
        }
        let base = self.api_endpoint.trim_end_matches('/');
        if base.starts_with("https://") {
            format!("wss://{}", &base["https://".len()..])
        } else if base.starts_with("http://") {
            format!("ws://{}", &base["http://".len()..])
        } else {
            format!("wss://{}", base)
        }
    }

    /// Resolve the effective timeout: provider-specific or global default.
    pub fn resolved_timeout_seconds(&self, global_default: u64) -> u64 {
        self.request_timeout_seconds.unwrap_or(global_default)
    }

    /// Resolve HTTP headers merging static headers with env-var-sourced headers.
    pub fn resolved_http_headers(&self) -> BTreeMap<String, String> {
        let mut headers = self.http_headers.clone();
        for (header_name, env_key) in &self.env_http_headers {
            if env_key.trim().is_empty() {
                continue;
            }
            if let Ok(value) = std::env::var(env_key) {
                let value = value.trim().to_string();
                if !value.is_empty() {
                    headers.insert(header_name.clone(), value);
                }
            }
        }
        headers
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct ProviderModelConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Optional user-facing friendly name.
    /// When absent, the model ID (map key) is shown instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Optional protocol override for this model.
    /// When set (e.g., "chat_completions", "embeddings"), the framework
    /// uses this protocol instead of auto-detecting from the provider kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api_protocol: Option<String>,
    #[serde(default, skip_serializing_if = "ModelRequestConfig::is_default")]
    pub request: ModelRequestConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, JsonSchema)]
#[serde(default)]
pub struct ModelRequestConfig {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_p: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub top_k: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_output_tokens: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frequency_penalty: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub presence_penalty: Option<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<crate::provider_api::types::ReasoningConfig>,
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub extra_body: BTreeMap<String, Value>,
}

impl ModelRequestConfig {
    /// Returns true if all fields are default/empty, meaning nothing was overridden.
    pub fn is_default(&self) -> bool {
        self.temperature.is_none()
            && self.top_p.is_none()
            && self.top_k.is_none()
            && self.max_output_tokens.is_none()
            && self.frequency_penalty.is_none()
            && self.presence_penalty.is_none()
            && self.reasoning.is_none()
            && self.extra_body.is_empty()
    }
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
    pub api_protocol: Option<String>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            language: default_language(),
            active_agent_id: default_active_agent_id(),
            providers: BTreeMap::new(),
            mcp: McpConfig::default(),
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
            credential: None,
            credential_env: None,
            api_endpoint: String::new(),
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            query_params: BTreeMap::new(),
            models_endpoint_candidates: Vec::new(),
            realtime_endpoint: None,
            supports_websockets: true,
            stream_transport: "sse".to_string(),
            request_timeout_seconds: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            extension_fields: HashMap::new(),
        }
    }
}

impl Default for ProviderModelConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            name: None,
            api_protocol: None,
            request: ModelRequestConfig::default(),
        }
    }
}
