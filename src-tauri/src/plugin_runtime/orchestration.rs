use super::SandboxPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Source of a tool call in the agent pipeline.
///
/// Keeping the source explicit lets the orchestrator route native, plugin, and
/// MCP-backed calls through the right execution path later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallSource {
    Native,
    Plugin { plugin_id: String },
    Mcp { server_id: String },
}

/// A single tool call request handed to the orchestrator.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallRequest {
    pub call_id: String,
    pub tool_name: String,
    pub arguments: Value,
    pub source: ToolCallSource,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hop_index: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sandbox: Option<SandboxPolicy>,
}

/// Execution policy for a batch of tool calls.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallExecutionPolicy {
    pub allow_parallel: bool,
    pub max_parallelism: usize,
    pub max_retries: usize,
    pub guard_repeated_failures: bool,
}

/// A normalized batch of tool calls ready for execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallBatch {
    pub requests: Vec<ToolCallRequest>,
    pub policy: ToolCallExecutionPolicy,
}

/// Outcome of a single tool call.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolCallOutcome {
    pub call_id: String,
    pub tool_name: String,
    pub source: ToolCallSource,
    pub ok: bool,
    pub output: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at_unix_ms: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

impl ToolCallBatch {
    /// Create a single-call batch with conservative execution defaults.
    pub fn single(request: ToolCallRequest) -> Self {
        Self {
            requests: vec![request],
            policy: ToolCallExecutionPolicy {
                allow_parallel: false,
                max_parallelism: 1,
                max_retries: 2,
                guard_repeated_failures: true,
            },
        }
    }
}
