use serde_json::Value;
use std::collections::BTreeMap;

/// Normalized model-facing metadata for a mounted dynamic tool.
#[derive(Debug, Clone, PartialEq)]
pub struct MountedToolDefinition {
    pub tool_name: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub input_schema: Value,
}

/// Conversation-scoped mounted tool source plus the data needed to rebuild it.
#[derive(Debug, Clone)]
pub struct MountedToolSourceSession {
    pub source_id: String,
    pub source_type: String,
    pub tools: Vec<MountedToolDefinition>,
    pub mcp_config: Option<crate::config::McpServerConfig>,
}

pub type MountedToolSourceSessions = BTreeMap<String, MountedToolSourceSession>;

/// Persistent state changes requested by a tool execution.
#[derive(Debug, Clone)]
pub enum ToolCatalogStateChange {
    MountToolSource(MountedToolSourceSession),
    UnmountToolSource {
        source_id: String,
        #[allow(dead_code)] // Reserved for future use
        source_type: String,
    },
}

/// A tool result plus catalog side effects that the chat command persists.
#[derive(Debug, Clone)]
pub(crate) struct ToolCatalogExecution {
    pub output: Value,
    pub state_changes: Vec<ToolCatalogStateChange>,
}
