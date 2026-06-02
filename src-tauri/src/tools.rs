pub(crate) mod background_jobs;
mod calculator;
mod catalog;
mod files;
pub(crate) mod memory_tools;
mod native;
mod registry;
pub(crate) mod sub_agent_tools;

use crate::config::AppConfig;
use crate::error::AgentJaxResult;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::sync::Arc;

pub(crate) use catalog::ToolCatalogExecution;
pub use catalog::{
    DeclaredPermissions, EffectivePermissions, MountedToolDefinition, MountedToolSourceSession,
    MountedToolSourceSessions, PluginEntryPolicyPaths, PluginEntrySnapshot, PluginManagerSnapshot,
    ToolCatalog, ToolCatalogSnapshot, ToolCatalogStateChange, ToolManagerSchemaFormat,
    ToolManagerSnapshot, ToolManagerSnapshotRequest, ToolManagerSourceSnapshot,
    ToolManagerSourceType, ToolManagerToolSnapshot, build_plugin_manager_snapshot,
};
pub use files::{EditFileTool, FileReaderTool, FileWriterTool, ListFilesTool, MkdirTool};
pub use native::{CalculatorTool, SystemTimeTool};
pub use registry::ToolRegistry;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolSchemaFormat {
    Responses,
    ChatCompletions,
    Gemini,
    Anthropic,
}

pub fn format_tool_schema(
    format: ToolSchemaFormat,
    name: &str,
    description: &str,
    parameters: Value,
) -> Value {
    match format {
        ToolSchemaFormat::Responses => json!({
            "type": "function",
            "name": name,
            "description": description,
            "parameters": parameters,
        }),
        ToolSchemaFormat::ChatCompletions => json!({
            "type": "function",
            "function": {
                "name": name,
                "description": description,
                "parameters": parameters,
            }
        }),
        ToolSchemaFormat::Gemini => json!({
            "name": name,
            "description": description,
            "parameters": parameters,
        }),
        ToolSchemaFormat::Anthropic => json!({
            "name": name,
            "description": description,
            "input_schema": parameters,
        }),
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct ToolPresentation {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
}

impl ToolPresentation {
    pub fn new(
        display_name: impl Into<String>,
        description: impl Into<String>,
        icon: Option<impl Into<String>>,
    ) -> Self {
        Self {
            display_name: display_name.into(),
            description: description.into(),
            icon: icon.map(Into::into),
        }
    }
}

pub fn humanize_tool_name(name: &str) -> String {
    name.split(['_', '-', '.'])
        .filter(|part| !part.trim().is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => first.to_ascii_uppercase().to_string() + chars.as_str(),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[async_trait::async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &'static str;
    fn description(&self) -> &'static str;
    fn parameters_schema(&self) -> Value;
    async fn execute(&self, arguments: &Value, context: &ToolExecutionContext) -> AgentJaxResult<Value>;
    fn display_name(&self) -> &'static str {
        self.name()
    }
    fn icon(&self) -> Option<&'static str> {
        None
    }

    fn presentation(&self) -> ToolPresentation {
        ToolPresentation::new(
            self.display_name(),
            self.description(),
            self.icon().map(str::to_string),
        )
    }

    fn to_schema_with_format(&self, format: ToolSchemaFormat) -> Value {
        format_tool_schema(
            format,
            self.name(),
            self.description(),
            self.parameters_schema(),
        )
    }

    fn to_schema(&self) -> Value {
        self.to_schema_with_format(ToolSchemaFormat::Responses)
    }
}

#[derive(Clone, Default)]
pub struct ToolExecutionContext {
    pub conversation_id: Option<String>,

    /// The model identifier being used for this request.
    pub model_id: Option<String>,

    /// Current turn identifier.
    pub turn_id: Option<String>,

    /// Current hop index in the tool-call loop.
    pub hop_index: Option<u32>,

    /// Application configuration (for tools that need provider access).
    pub app_config: Option<Arc<AppConfig>>,

    /// Sub-agent identifier — set when executing within an async sub-agent.
    /// This allows tools like `lcm_expand` to distinguish between main-agent
    /// and sub-agent contexts.
    pub sub_agent_id: Option<String>,

    /// Override LCM store for sub-agent contexts.
    /// When set, LCM tools (lcm_grep, lcm_describe, lcm_expand) use this
    /// store instead of the parent's store, ensuring sub-agents operate
    /// on their own isolated conversation history.
    #[allow(dead_code)]
    pub lcm_store_override: Option<Arc<crate::lcm::LcmStore>>,

    /// The sub-agent type when executing in a sub-agent context.
    /// Used to gate tools that are exclusive to specific sub-agent types
    /// (e.g., memory_write is only available to the Memory sub-agent).
    pub sub_agent_type: Option<String>,

    /// Whether this execution context belongs to the background memory sub-agent.
    /// When true, the memory_write tool is available.
    pub is_memory_sub_agent: bool,

    /// Channel sender for sub-agent lifecycle events.
    /// Populated in the chat stream handler so the sub-agent runner can push
    /// progress/completion events to the frontend.
    pub sub_agent_event_tx: Option<tokio::sync::mpsc::UnboundedSender<crate::sub_agents::SubAgentEvent>>,
}

// Manual Debug impl that skips lcm_store_override (LcmStore doesn't impl Debug).
impl std::fmt::Debug for ToolExecutionContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolExecutionContext")
            .field("conversation_id", &self.conversation_id)
            .field("model_id", &self.model_id)
            .field("turn_id", &self.turn_id)
            .field("hop_index", &self.hop_index)
            .field("app_config", &self.app_config)
            .field("sub_agent_id", &self.sub_agent_id)
            .field("lcm_store_override", &self.lcm_store_override.as_ref().map(|_| "Arc<LcmStore>"))
            .field("sub_agent_type", &self.sub_agent_type)
            .field("is_memory_sub_agent", &self.is_memory_sub_agent)
            .field("sub_agent_event_tx", &self.sub_agent_event_tx.as_ref().map(|_| "mpsc::UnboundedSender"))
            .finish()
    }
}

impl ToolExecutionContext {
    /// Create a new context with just a conversation ID (all other fields default).
    pub fn with_conversation_id(conversation_id: impl Into<String>) -> Self {
        Self {
            conversation_id: Some(conversation_id.into()),
            model_id: None,
            turn_id: None,
            hop_index: None,
            app_config: None,
            sub_agent_id: None,
            lcm_store_override: None,
            sub_agent_type: None,
            is_memory_sub_agent: false,
            sub_agent_event_tx: None,
        }
    }
}

// ── Scope-Narrowing Invariant (LCM §3.2) ─────────────────────────────────────

/// Check the scope-narrowing invariant for sub-agent delegation.
///
/// When a sub-agent (non-root) spawns a further sub-agent, it must declare
/// both `delegated_scope` (which tools the sub-agent may access) and
/// `kept_work` (what output the sub-agent will produce). If the caller cannot
/// articulate what it is keeping, the engine rejects the delegation.
///
/// ## Exemptions
/// - **Root agents** (`hop_index == 0`): the main agent can spawn sub-agents
///   without declaring scope/work.
/// - **Explore sub-agents** (`subagent_type == "explore"`): read-only agents
///   that cannot spawn further sub-agents and are exempt.
///
/// This structural guarantee ensures each level of delegation represents a
/// strict reduction in responsibility, creating a well-founded recursion that
/// must eventually bottom out in direct execution.
pub fn check_scope_narrowing_invariant(
    subagent_type: &Option<String>,
    delegated_scope: &[String],
    kept_work: &[String],
    context: &ToolExecutionContext,
) -> Result<(), crate::error::AgentJaxError> {
    let is_explore = subagent_type
        .as_deref()
        .map(|t| t == "explore")
        .unwrap_or(false);
    let is_memory = subagent_type
        .as_deref()
        .map(|t| t == "memory")
        .unwrap_or(false);
    let is_root = context.hop_index.unwrap_or(0) == 0;
    let is_sub_agent = context.sub_agent_id.is_some();

    // Root agents, explore sub-agents, and memory sub-agents are exempt.
    // Memory sub-agents have a fixed, well-known scope (memory write only).
    if is_root || is_explore || is_memory || is_sub_agent {
        return Ok(());
    }

    if kept_work.is_empty() {
        return Err(crate::error::AgentJaxError::sub_agent(
            "Scope-narrowing invariant violation: sub-agent must declare non-empty \
             'kept_work' — describe what concrete output you will produce. \
             Without this, the delegation would represent a pass-through with \
             no reduction in responsibility."
                .to_string(),
        ));
    }
    if delegated_scope.is_empty() {
        return Err(crate::error::AgentJaxError::sub_agent(
            "Scope-narrowing invariant violation: sub-agent must declare non-empty \
             'delegated_scope' — specify which tools the sub-agent may access."
                .to_string(),
        ));
    }
    Ok(())
}
