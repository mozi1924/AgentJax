//! Core types for the sub-agent system.
//!
//! These types define the sub-agent lifecycle: spec → spawn → monitor → collect.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Sub-Agent Type ────────────────────────────────────────────────────────────

/// The kind of sub-agent, determining its tool access and behavior.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SubAgentType {
    /// Read-only exploration: filesystem viewing, no mutations.
    /// Exempt from scope-narrowing invariant.
    Explore,
    /// Code review and analysis, produces reports.
    CodeReview,
    /// Implementation: can write/edit files within scope.
    Implement,
    /// Data analysis and reasoning.
    Analyze,
    /// Full tool access within delegated scope.
    GeneralPurpose,
    /// Background memory observer: read-only context access,
    /// exclusive memory_write permission. Runs persistently
    /// (event-driven) and is exempt from scope-narrowing.
    Memory,
}

impl SubAgentType {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubAgentType::Explore => "explore",
            SubAgentType::CodeReview => "codeReview",
            SubAgentType::Implement => "implement",
            SubAgentType::Analyze => "analyze",
            SubAgentType::GeneralPurpose => "general",
            SubAgentType::Memory => "memory",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "explore" => Some(SubAgentType::Explore),
            "codeReview" => Some(SubAgentType::CodeReview),
            "implement" => Some(SubAgentType::Implement),
            "analyze" => Some(SubAgentType::Analyze),
            "general" => Some(SubAgentType::GeneralPurpose),
            "memory" => Some(SubAgentType::Memory),
            _ => None,
        }
    }
}

// ── Sub-Agent Status ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SubAgentStatus {
    /// Created but not yet running.
    Pending,
    /// Actively executing.
    Running,
    /// Finished successfully.
    Completed,
    /// Finished with error.
    Failed,
    /// Cancelled by caller.
    Cancelled,
}

impl SubAgentStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            SubAgentStatus::Pending => "pending",
            SubAgentStatus::Running => "running",
            SubAgentStatus::Completed => "completed",
            SubAgentStatus::Failed => "failed",
            SubAgentStatus::Cancelled => "cancelled",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            SubAgentStatus::Completed | SubAgentStatus::Failed | SubAgentStatus::Cancelled
        )
    }
}

// ── Sub-Agent Spec ────────────────────────────────────────────────────────────

/// The specification for spawning a new sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentSpec {
    /// Unique identifier for this sub-agent (UUID).
    pub agent_id: String,
    /// The conversation that spawned this sub-agent.
    pub parent_conversation_id: String,
    /// The type of sub-agent.
    pub subagent_type: SubAgentType,
    /// The task description / prompt for the sub-agent.
    pub prompt: String,
    /// Which tool domains the sub-agent may access.
    pub delegated_scope: Vec<String>,
    /// What concrete outputs the sub-agent is expected to produce.
    pub kept_work: Vec<String>,
    /// Maximum tool-using turns (default 5, max 10).
    pub max_turns: usize,
    /// Maximum retries on failure (default 0 = no retry).
    #[serde(default)]
    pub max_retries: u32,
    /// Whether to create an isolated git worktree.
    pub use_worktree: bool,
    /// Optional model override (defaults to utility_small_model).
    pub model_id: Option<String>,
    /// Links back to the spawning request.
    pub parent_request_id: String,
    /// Whether this sub-agent is persistent (survives TTL pruning,
    /// not force-cancelled on conversation end). Memory agents
    /// set this to true.
    #[serde(default)]
    pub persistent: bool,
}

// ── Progress Message ──────────────────────────────────────────────────────────

/// A textual progress update emitted by a running sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressMessage {
    /// The progress text.
    pub text: String,
    /// Unix timestamp in milliseconds.
    pub ts: i64,
}

// ── Sub-Agent State ───────────────────────────────────────────────────────────

/// The full runtime state of a sub-agent.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentState {
    pub agent_id: String,
    pub spec: SubAgentSpec,
    pub status: SubAgentStatus,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: Option<i64>,
    pub duration_ms: Option<u64>,
    pub result: Option<Value>,
    pub error: Option<String>,
    pub progress_messages: Vec<ProgressMessage>,
    /// Number of tool-using turns completed so far.
    pub turns_completed: usize,
}

// ── Snapshot (for Tauri IPC) ──────────────────────────────────────────────────

/// A lightweight snapshot of sub-agent state for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubAgentSnapshot {
    pub agent_id: String,
    pub parent_conversation_id: String,
    pub subagent_type: String,
    pub prompt: String,
    pub status: String,
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: Option<i64>,
    pub duration_ms: Option<u64>,
    pub turns_completed: usize,
    pub max_turns: usize,
    pub error: Option<String>,
}

// ── Memory Agent Signal ───────────────────────────────────────────────────────

/// Signals sent to the persistent background memory sub-agent.
#[derive(Debug, Clone)]
pub enum MemoryAgentSignal {
    /// A main-agent turn has completed. The memory agent should evaluate
    /// the conversation context and decide whether to write/update memories.
    TurnCompleted,
    /// The conversation has ended. The memory agent should perform a final
    /// evaluation and then exit.
    #[allow(dead_code)]
    Terminate,
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_type_as_str_roundtrip() {
        let variants = &[
            SubAgentType::Explore,
            SubAgentType::CodeReview,
            SubAgentType::Implement,
            SubAgentType::Analyze,
            SubAgentType::GeneralPurpose,
        ];
        for variant in variants {
            let s = variant.as_str();
            let parsed = SubAgentType::from_str(s);
            assert_eq!(parsed, Some(variant.clone()), "roundtrip failed for {s}");
        }
    }

    #[test]
    fn test_sub_agent_type_from_str_unknown() {
        assert_eq!(SubAgentType::from_str("nonexistent"), None);
    }

    #[test]
    fn test_sub_agent_status_terminal() {
        assert!(!SubAgentStatus::Pending.is_terminal());
        assert!(!SubAgentStatus::Running.is_terminal());
        assert!(SubAgentStatus::Completed.is_terminal());
        assert!(SubAgentStatus::Failed.is_terminal());
        assert!(SubAgentStatus::Cancelled.is_terminal());
    }

    #[test]
    fn test_spec_serialization() {
        let spec = SubAgentSpec {
            agent_id: "agent_001".to_string(),
            parent_conversation_id: "conv_001".to_string(),
            subagent_type: SubAgentType::Explore,
            prompt: "Find all test files".to_string(),
            delegated_scope: vec!["filesystem".to_string()],
            kept_work: vec!["file_list".to_string()],
            max_turns: 5,
            max_retries: 0,
            use_worktree: false,
            model_id: None,
            parent_request_id: "req_001".to_string(),
            persistent: false,
        };
        let json = serde_json::to_string(&spec).unwrap();
        let parsed: SubAgentSpec = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.agent_id, spec.agent_id);
        assert_eq!(parsed.subagent_type, spec.subagent_type);
        assert_eq!(parsed.max_turns, 5);
    }
}
