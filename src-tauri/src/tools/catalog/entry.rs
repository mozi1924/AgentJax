//! Tool entry types — unified tool registration metadata.
//!
//! Replaces the split between `native_tools` and `context_tools` in
//! `ToolCatalog` with a single `Vec<ToolEntry>` where each entry carries
//! its category and gating rules. This eliminates duplicated enumeration
//! logic across model snapshots and tool manager snapshots.
//!
//! ## Phase 2: Shared enumeration
//!
//! [`RegisteredToolInfo`] is an intermediate representation produced by
//! `ToolCatalog::collect_registered_tools()` and consumed by both the
//! model snapshot and the tool manager snapshot, ensuring identical
//! filtering and gating for native + context tools in both paths.

use crate::tools::Tool;
use serde_json::Value;
use std::sync::Arc;

// ── Tool Category ───────────────────────────────────────────────────────────

/// Whether a registered tool is a built-in native tool, a context tool
/// (LCM, memory), or a knowledge base tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCategory {
    /// Built-in tools — enablement controlled via `tool_manager.native_tools.*`.
    Native,
    /// Context/LCM/memory tools — forced enabled; some gated by the
    /// agent's memory configuration or sub-agent type.
    Context,
    /// Knowledge Base tools (kb_list, kb_search, kb_get, kb_index) —
    /// enablement controlled via `tool_manager.context_tools.*`.
    KnowledgeBase,
}

// ── Context Gating ──────────────────────────────────────────────────────────

/// Additional visibility gating for context tools.
///
/// Only applies when `category == ToolCategory::Context`.  Native tools
/// always use `None`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ContextGating {
    /// No additional gating — always available when context tools are enabled.
    None,

    /// Available only when the agent's memory system is enabled
    /// (`AgentConfig.memory.enabled`).
    ///
    /// Applied to: `memory_search`, `memory_recall`, `memory_write`.
    MemoryEnabled,

    /// Available only when executing inside a Memory sub-agent.
    ///
    /// Applied to: `memory_write`.
    MemorySubAgentOnly,
}

// ── ToolEntry ───────────────────────────────────────────────────────────────

/// A single registered tool in the catalog, annotated with category and
/// gating metadata.
pub(crate) struct ToolEntry {
    /// The tool implementation.
    pub tool: Arc<dyn Tool>,

    /// Whether this is a native or context tool.
    pub category: ToolCategory,

    /// Optional gating rules for context tools.
    pub context_gating: ContextGating,
}

impl ToolEntry {
    /// Create a native tool entry (no gating).
    pub(crate) fn native(tool: Arc<dyn Tool>) -> Self {
        Self {
            tool,
            category: ToolCategory::Native,
            context_gating: ContextGating::None,
        }
    }

    /// Create a context tool entry with the given gating rule.
    pub(crate) fn context(tool: Arc<dyn Tool>, gating: ContextGating) -> Self {
        Self {
            tool,
            category: ToolCategory::Context,
            context_gating: gating,
        }
    }

    /// Convenience: context tool with no gating.
    pub(crate) fn context_always(tool: Arc<dyn Tool>) -> Self {
        Self::context(tool, ContextGating::None)
    }

    /// Create a knowledge base tool entry (disableable, no context gating).
    pub(crate) fn kb(tool: Arc<dyn Tool>) -> Self {
        Self {
            tool,
            category: ToolCategory::KnowledgeBase,
            context_gating: ContextGating::None,
        }
    }

    /// Proxy to the underlying tool's name.
    pub(crate) fn name(&self) -> &str {
        self.tool.name()
    }

    /// Proxy to the underlying tool's display name.
    pub(crate) fn display_name(&self) -> &str {
        self.tool.display_name()
    }

    /// Proxy to the underlying tool's description.
    pub(crate) fn description(&self) -> &str {
        self.tool.description()
    }

    /// Proxy to the underlying tool's icon.
    pub(crate) fn icon(&self) -> Option<&str> {
        self.tool.icon()
    }
}

// ── RegisteredToolInfo (shared enumeration output) ──────────────────────────

/// Intermediate representation of a registered (native or context) tool,
/// produced by `ToolCatalog::collect_registered_tools()`.
///
/// Both the model snapshot and the tool manager snapshot consume this same
/// struct, ensuring identical filtering and gating for native + context tools.
#[derive(Clone)]
pub(crate) struct RegisteredToolInfo {
    pub name: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    /// The JSON schema (parameters) for the tool.
    pub schema: Value,
    /// The underlying tool implementation.
    pub tool: Arc<dyn Tool>,
    /// Whether this tool's category is Native or Context.
    pub category: ToolCategory,
    /// Whether the tool passes the base enablement policy check.
    pub enabled: bool,
}

impl RegisteredToolInfo {
    pub(crate) fn from_entry(entry: &ToolEntry, enabled: bool) -> Self {
        Self {
            name: entry.name().to_string(),
            display_name: entry.display_name().to_string(),
            description: entry.description().to_string(),
            icon: entry.icon().map(ToOwned::to_owned),
            schema: entry.tool.parameters_schema(),
            tool: entry.tool.clone(),
            category: entry.category,
            enabled,
        }
    }
}

// ── PluginCollectedTool (shared enumeration output) ─────────────────────────

/// Intermediate representation of a plugin tool, produced by
/// `ToolCatalog::collect_plugin_tools()`.
#[derive(Clone)]
pub(crate) struct PluginCollectedTool {
    pub plugin_id: String,
    pub tool_name: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub input_schema: Value,
    pub enabled: bool,
}

// ── McpCollectedTool (shared enumeration output) ────────────────────────────

/// Intermediate representation of a collected MCP tool, produced when
/// enumerating MCP server tools for either the model snapshot or the
/// tool manager snapshot.
///
/// The `server_id` is not stored here because callers already have it
/// in their scope — this struct carries only per-tool fields.
#[derive(Clone)]
pub(crate) struct McpCollectedTool {
    pub tool_name: String,
    pub display_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub input_schema: Value,
    pub enabled: bool,
}
