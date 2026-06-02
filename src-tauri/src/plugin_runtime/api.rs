use super::{PluginManifest, PluginToolDefinition, SandboxPolicy};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Current host/plugin API contract version.
///
/// Plugin manifests pin to this number so the host can reject plugins that were
/// authored for an incompatible JavaScript bridge before loading any code.
pub const PLUGIN_API_VERSION: u32 = 1;
#[allow(dead_code)] // Reserved for future use
pub const PLUGIN_SOURCE_TYPE: &str = "plugin";
pub const PLUGIN_TOOL_NAME_PREFIX: &str = "plugin__";

/// A plugin tool after it has been accepted into the host registry.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct RegisteredPluginTool {
    pub plugin_id: String,
    pub plugin_name: String,
    pub tool: PluginToolDefinition,
}

impl RegisteredPluginTool {
    /// Build the model-visible tool name used by the shared tool catalog.
    #[allow(dead_code)] // Test-only API surface
    pub fn prefixed_name(&self) -> String {
        prefixed_plugin_tool_name(&self.plugin_id, &self.tool.name)
    }
}

/// Conversation metadata passed to plugin tool handlers.
///
/// This carries contextual information about the current invocation so plugins
/// can make informed decisions about context access, token budgets, and
/// conversation state without requiring additional RPC.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct PluginInvocationContext {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,

    /// The model identifier being used for this request (e.g. "gpt-4o").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model_id: Option<String>,

    /// Current turn identifier, set when the runtime processes a user message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,

    /// Current hop index in the tool-call loop (0-based). `None` when not
    /// inside a tool-call continuation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop_index: Option<u32>,

    /// Estimated token count of the assembled request context (approximate).
    /// This is a rough estimate for budgeting, not an exact tokenizer count.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_token_estimate: Option<usize>,

    /// Number of conversation messages (lines) loaded for this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message_count: Option<usize>,

    /// Number of tool call entries in the assembled context.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call_count: Option<usize>,
}

/// Host-normalized request for a plugin tool invocation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // Reserved for future use
pub struct PluginToolCall {
    pub plugin_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub context: PluginInvocationContext,
    pub sandbox: SandboxPolicy,
}

/// Result shape returned by plugin handlers before the agent runtime wraps it
/// in provider-specific tool output metadata.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolResult {
    pub ok: bool,
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[allow(dead_code)] // Reserved for future use
impl PluginToolResult {
    pub fn success(output: Value) -> Self {
        Self {
            ok: true,
            output,
            error: None,
        }
    }

    pub fn failure(message: impl Into<String>) -> Self {
        Self {
            ok: false,
            output: Value::Null,
            error: Some(message.into()),
        }
    }
}

/// Return all tool declarations exported by a manifest with plugin identity
/// attached. This is the normalization step used before catalog mounting.
pub fn registered_tools_for_manifest(manifest: &PluginManifest) -> Vec<RegisteredPluginTool> {
    manifest
        .tools
        .iter()
        .cloned()
        .map(|tool| RegisteredPluginTool {
            plugin_id: manifest.id.clone(),
            plugin_name: manifest.name.clone(),
            tool,
        })
        .collect()
}

/// Convert plugin identity and local tool name into a provider-safe function
/// name. The original manifest values remain intact for dispatch.
pub fn prefixed_plugin_tool_name(plugin_id: &str, tool_name: &str) -> String {
    format!(
        "{PLUGIN_TOOL_NAME_PREFIX}{}__{}",
        sanitize_tool_name_segment(plugin_id),
        sanitize_tool_name_segment(tool_name)
    )
}

fn sanitize_tool_name_segment(value: &str) -> String {
    let sanitized = value
        .trim()
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();

    if sanitized.is_empty() {
        "unnamed".to_string()
    } else {
        sanitized
    }
}
