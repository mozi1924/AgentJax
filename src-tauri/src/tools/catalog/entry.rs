//! Tool entry types — unified tool registration metadata.
//!
//! Replaces the split between `native_tools` and `context_tools` in
//! `ToolCatalog` with a single `Vec<ToolEntry>` where each entry carries
//! its category and gating rules. This eliminates duplicated enumeration
//! logic across model snapshots and tool manager snapshots.

use crate::tools::Tool;
use std::sync::Arc;

// ── Tool Category ───────────────────────────────────────────────────────────

/// Whether a registered tool is a built-in native tool or a context tool
/// (LCM, memory, knowledge base).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ToolCategory {
    /// Built-in tools — enablement controlled via `tool_manager.native_tools.*`.
    Native,
    /// Context/LCM/memory/KB tools — forced enabled; some gated by the
    /// agent's memory configuration or sub-agent type.
    Context,
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
