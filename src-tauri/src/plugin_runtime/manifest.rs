use super::SandboxPolicy;
use super::api::PLUGIN_API_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Tool declaration exported by a plugin.
///
/// This mirrors the shape we already use for native and MCP-backed tools so
/// plugin tools can be normalized into the same catalog later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "default_input_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub kind: PluginToolKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginToolKind {
    #[default]
    Function,
    Resource,
    Prompt,
}

// ── Glob-based Model Routing ───────────────────────────────────────────────

/// A single rule mapping a model ID glob pattern to a protocol + API path.
///
/// Rules are evaluated in declaration order; the first match wins.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ModelRoutingRule {
    /// Glob pattern to match against model ID (e.g. `"gpt-5*"`, `"text-embedding*"`, `"*"`).
    pub pattern: String,
    /// Protocol name to route to (e.g. `"chat_completions"`, `"responses"`, `"embeddings"`).
    pub protocol: String,
    /// API path suffix appended to the base URL (e.g. `"/v1/chat/completions"`).
    pub path: String,
}

// ── Built-in Model Descriptor ─────────────────────────────────────────────

/// Describes a model known to a provider at build time.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BuiltinModelDescriptor {
    /// Model ID (e.g. `"gpt-5-mini"`).
    pub id: String,
    /// Model kind (`"chat"`, `"embedding"`, `"reasoning"`, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// Context window size in tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<usize>,
    /// Supported reasoning levels for this model.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub supported_reasoning_levels: Option<Vec<String>>,
}

// ── Auth Strategy Declaration ─────────────────────────────────────────────

/// Authentication strategy that the host applies server-side.
///
/// The credential is injected by Rust after the JS plugin returns its HTTP
/// request definition, so the raw API key never enters the V8 runtime.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    /// Auth type: `"api_key"`, `"bearer"`, `"basic"`, `"custom_header"`.
    #[serde(default = "default_auth_type")]
    pub auth_type: String,
    /// Environment variable name to read the credential from.
    pub credential_env: String,
    /// Where and how to place the credential in requests.
    pub placement: AuthPlacement,
}

fn default_auth_type() -> String {
    "api_key".to_string()
}

/// Describes where to place the credential in an HTTP request.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct AuthPlacement {
    /// Location: `"header"` or `"query"`.
    #[serde(default = "default_auth_in")]
    pub in_field: String,
    /// Header name or query parameter name.
    pub key: String,
    /// Format string with `"{key}"` placeholder (e.g. `"Bearer {key}"`, `"ApiKey {key}"`).
    pub format: String,
}

fn default_auth_in() -> String {
    "header".to_string()
}

/// Declarative provider definition exported by a model provider plugin.
///
/// All fields except `kind` and `display_name` are optional. The provider
/// can be fully declared in JSON for standard API shapes; only providers
/// requiring custom request/response logic need a JS entrypoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginProviderDefinition {
    pub kind: String,
    pub display_name: String,
    #[serde(default = "default_config_schema")]
    pub config_schema: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_model_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_schema_format: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub supports_protocols: Vec<String>,

    // ── New Phase 2 fields (declarative, no JS needed) ──────────────────
    /// Ordered glob-based model routing rules.
    /// When non-empty, used instead of `supports_protocols` heuristics.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub model_routing: Vec<ModelRoutingRule>,

    /// Built-in model descriptors known at build time.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub builtin_models: Vec<BuiltinModelDescriptor>,

    /// Authentication strategy declaration.
    /// When present, the host applies credentials server-side.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub auth: Option<AuthConfig>,

    /// Declarative reasoning/thinking schema.
    ///
    /// Tells the framework what extra_body fields to inject when reasoning
    /// mode is enabled for this provider. This is in addition to standard
    /// protocol-level reasoning fields (e.g. `reasoning_effort` for Chat
    /// Completions, `reasoning` object for Responses API).
    ///
    /// Example (DeepSeek needs `thinking: {"type": "enabled"}`):
    /// ```json
    /// {"enabledExtraBody": {"thinking": {"type": "enabled"}}}
    /// ```
    ///
    /// When absent or empty, no extra reasoning fields are injected and the
    /// framework relies solely on standard protocol-level reasoning handling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_schema: Option<ReasoningSchema>,
}

impl Default for PluginProviderDefinition {
    fn default() -> Self {
        Self {
            kind: String::new(),
            display_name: String::new(),
            config_schema: default_config_schema(),
            default_model_ids: Vec::new(),
            default_priority: None,
            capabilities: None,
            tool_schema_format: None,
            supports_protocols: Vec::new(),
            model_routing: Vec::new(),
            builtin_models: Vec::new(),
            auth: None,
            reasoning_schema: None,
        }
    }
}

fn default_config_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

/// Declarative plugin metadata that the host can validate before loading code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_api_version")]
    pub api_version: u32,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PluginToolDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_sections: Vec<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings_data: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<PluginProviderDefinition>,
    #[serde(default)]
    pub sandbox: SandboxPolicy,
}

fn default_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

// ── Reasoning Schema ───────────────────────────────────────────────────────

/// Declarative schema for how a provider handles reasoning/thinking mode.
///
/// Each provider can declare what extra HTTP body fields to inject when
/// reasoning is enabled. Standard protocol-level reasoning fields
/// (`reasoning_effort`, `reasoning` object, etc.) are handled by the protocol
/// implementations (`chat.rs`, `responses.rs`) and do NOT need to be declared
/// here. This schema only covers **additional** provider-specific fields.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ReasoningSchema {
    /// Extra body fields to merge into the request payload when reasoning is
    /// enabled. The framework deep-merges these into `extra_body` before the
    /// payload is serialized.
    ///
    /// For DeepSeek:
    /// ```json
    /// {"thinking": {"type": "enabled"}}
    /// ```
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub enabled_extra_body: BTreeMap<String, Value>,
}

impl Default for ReasoningSchema {
    fn default() -> Self {
        Self {
            enabled_extra_body: BTreeMap::new(),
        }
    }
}

fn default_api_version() -> u32 {
    PLUGIN_API_VERSION
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: String::new(),
            description: String::new(),
            tools: Vec::new(),
            settings_sections: Vec::new(),
            settings_data: BTreeMap::new(),
            providers: Vec::new(),
            sandbox: SandboxPolicy::default(),
        }
    }
}

impl PluginManifest {
    /// Validate a manifest before the host accepts it into the registry.
    pub fn validate(&self) -> super::PluginRuntimeResult<()> {
        if self.id.trim().is_empty() {
            return Err(super::PluginRuntimeError::InvalidManifest(
                "plugin id cannot be empty".to_string(),
            ));
        }
        if self.name.trim().is_empty() {
            return Err(super::PluginRuntimeError::InvalidManifest(format!(
                "plugin '{}' is missing a name",
                self.id
            )));
        }
        if self.entrypoint.trim().is_empty() {
            return Err(super::PluginRuntimeError::InvalidManifest(format!(
                "plugin '{}' is missing an entrypoint",
                self.id
            )));
        }
        if self.api_version != PLUGIN_API_VERSION {
            return Err(super::PluginRuntimeError::InvalidManifest(format!(
                "plugin '{}' uses unsupported apiVersion {} (host supports {})",
                self.id, self.api_version, PLUGIN_API_VERSION
            )));
        }

        let mut tool_names = HashSet::new();
        for tool in &self.tools {
            let tool_name = tool.name.trim();
            if tool_name.is_empty() {
                return Err(super::PluginRuntimeError::InvalidManifest(format!(
                    "plugin '{}' exports a tool with an empty name",
                    self.id
                )));
            }
            if !tool_names.insert(tool_name.to_string()) {
                return Err(super::PluginRuntimeError::InvalidManifest(format!(
                    "plugin '{}' exports the tool '{}' more than once",
                    self.id, tool_name
                )));
            }
        }

        let mut provider_kinds = HashSet::new();
        for provider in &self.providers {
            let kind = provider.kind.trim().to_lowercase();
            if kind.is_empty() {
                return Err(super::PluginRuntimeError::InvalidManifest(format!(
                    "plugin '{}' exports a model provider with an empty kind",
                    self.id
                )));
            }
            if provider.display_name.trim().is_empty() {
                return Err(super::PluginRuntimeError::InvalidManifest(format!(
                    "plugin '{}' exports the model provider '{}' without a display name",
                    self.id, provider.kind
                )));
            }
            if !provider_kinds.insert(kind.clone()) {
                return Err(super::PluginRuntimeError::InvalidManifest(format!(
                    "plugin '{}' exports the model provider '{}' more than once",
                    self.id, provider.kind
                )));
            }
        }

        validate_settings_sections(&self.id, &self.settings_sections)?;

        Ok(())
    }
}

fn validate_settings_sections(
    plugin_id: &str,
    sections: &[Value],
) -> super::PluginRuntimeResult<()> {
    let mut ids = HashSet::new();
    for section in sections {
        walk_settings_node(plugin_id, section, &mut ids)?;
    }
    Ok(())
}

fn walk_settings_node(
    plugin_id: &str,
    node: &Value,
    ids: &mut HashSet<String>,
) -> super::PluginRuntimeResult<()> {
    let object = node.as_object().ok_or_else(|| {
        super::PluginRuntimeError::InvalidManifest(format!(
            "plugin '{plugin_id}' settings schema node must be an object"
        ))
    })?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            super::PluginRuntimeError::InvalidManifest(format!(
                "plugin '{plugin_id}' settings schema node is missing a non-empty id"
            ))
        })?;
    if !ids.insert(id.to_string()) {
        return Err(super::PluginRuntimeError::InvalidManifest(format!(
            "plugin '{plugin_id}' settings schema contains duplicate node id '{id}'"
        )));
    }

    if let Some(children) = object.get("children") {
        let children = children.as_array().ok_or_else(|| {
            super::PluginRuntimeError::InvalidManifest(format!(
                "plugin '{plugin_id}' settings schema node '{id}' children must be an array"
            ))
        })?;
        for child in children {
            walk_settings_node(plugin_id, child, ids)?;
        }
    }

    if let Some(tabs) = object.get("tabs") {
        let tabs = tabs.as_array().ok_or_else(|| {
            super::PluginRuntimeError::InvalidManifest(format!(
                "plugin '{plugin_id}' settings schema node '{id}' tabs must be an array"
            ))
        })?;
        for tab in tabs {
            if let Some(children) = tab.get("children") {
                let children = children.as_array().ok_or_else(|| {
                    super::PluginRuntimeError::InvalidManifest(format!(
                        "plugin '{plugin_id}' settings schema tab in node '{id}' children must be an array"
                    ))
                })?;
                for child in children {
                    walk_settings_node(plugin_id, child, ids)?;
                }
            }
        }
    }

    if let Some(item_template) = object.get("itemTemplate") {
        walk_settings_node(plugin_id, item_template, ids)?;
    }

    Ok(())
}
