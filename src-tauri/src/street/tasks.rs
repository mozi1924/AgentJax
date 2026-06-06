//! Task Record persistence — permanent execution results for async tasks.
//!
//! Stores the **final output** of every async task (sub-agents, background jobs)
//! in `tasks.jsonl` alongside the conversation's other artifacts. This enables
//! full traceability: the main agent can query past task results at any time,
//! even after app restarts.
//!
//! ## What's stored
//!
//! Only the **final result** is persisted — not the internal tool calls, thinking
//! chains, or intermediate hops. The sub-agent's full internal context is noise
//! for the main agent; only the output summary matters.
//!
//! ## File format (JSONL)
//!
//! One `TaskRecord` per line, append-only:
//!
//! ```jsonl
//! {"id":"agent_xxx","kind":"subAgent","conversationId":"conv-1","status":"completed",
//!  "summary":"Explored codebase, found 5 files matching pattern",
//!  "subagentType":"explore","prompt":"Find all TODO comments",
//!  "startedAtUnixMs":...,"completedAtUnixMs":...,"durationMs":1500,
//!  "turnsCompleted":3,"result":{...}}
//! ```

use crate::conversation_store::conversation_dir_path;
use crate::error::AgentJaxResult;
use crate::jsonl_store;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::PathBuf;

const TASKS_FILE_NAME: &str = "tasks.jsonl";

// ── Task Kind ───────────────────────────────────────────────────────────────

/// The kind of async task.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum TaskKind {
    SubAgent,
    BackgroundJob,
}

// ── TaskRecord ──────────────────────────────────────────────────────────────

/// A single task execution record, persisted to `tasks.jsonl`.
///
/// Only the **final result** is captured — internal tool calls, thinking
/// chains, and intermediate hops are deliberately excluded.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskRecord {
    /// Unique task identifier (e.g. `agent_xxx` or `job_xxx`).
    pub id: String,
    /// What kind of task this is.
    pub kind: TaskKind,
    /// The conversation that owns this task.
    pub conversation_id: String,
    /// Terminal status: completed, failed, cancelled.
    pub status: String,
    /// Human-readable summary of what the task did.
    pub summary: String,

    // ── SubAgent-specific fields ─────────────────────────────────────
    /// The sub-agent type (explore, implement, analyze, etc.).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_type: Option<String>,
    /// The original prompt given to the sub-agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt: Option<String>,

    // ── BackgroundJob-specific fields ────────────────────────────────
    /// The native tool/plugin name that ran in the background.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_name: Option<String>,

    // ── Timing ───────────────────────────────────────────────────────
    pub started_at_unix_ms: i64,
    pub completed_at_unix_ms: i64,
    pub duration_ms: u64,
    /// Number of tool-using turns completed (sub-agents only).
    #[serde(default)]
    pub turns_completed: usize,

    // ── Result ───────────────────────────────────────────────────────
    /// The final output value (parsed JSON or wrapped text).
    /// This is the sub-agent's final answer, not its internal tool calls.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    /// Error message if the task failed or was cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

// ── Path resolution ─────────────────────────────────────────────────────────

/// Resolve the path to `tasks.jsonl` for a given conversation.
///
/// Lives alongside `metadata.json`, `messages.jsonl`, `lcm.db`, and
/// `notifications.jsonl` in the conversation's session directory:
///
/// `~/.agentjax/agents/{agent_id}/sessions/{conv_id}/tasks.jsonl`
pub fn task_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    let dir = conversation_dir_path(
        crate::config::constants::DEFAULT_AGENT_ID,
        conversation_id,
    )?;
    Ok(dir.join(TASKS_FILE_NAME))
}

// ── Write ───────────────────────────────────────────────────────────────────

/// Append a task record to the conversation's `tasks.jsonl`.
///
/// This is called when a sub-agent or background job reaches a terminal
/// state (completed, failed, cancelled). The file is append-only so the
/// full execution history is preserved.
pub fn append_task(record: &TaskRecord) -> AgentJaxResult<()> {
    let path = task_path(&record.conversation_id)?;
    jsonl_store::append_line(&path, record, "task")?;
    Ok(())
}

// ── Read ────────────────────────────────────────────────────────────────────

// ── Builder helpers ─────────────────────────────────────────────────────────

/// Build a summary string from a sub-agent's spec and result.
pub fn sub_agent_summary(
    subagent_type: &str,
    prompt: &str,
    result: Option<&Value>,
    error: Option<&str>,
    status: &str,
) -> String {
    match status {
        "completed" => {
            let prompt_preview: String = prompt.chars().take(120).collect();
            if let Some(r) = result {
                let result_str = serde_json::to_string(r).unwrap_or_default();
                let result_truncated: String = result_str.chars().take(200).collect();
                format!(
                    "[{subagent_type}] {prompt_preview}… → {result_truncated}"
                )
            } else {
                format!("[{subagent_type}] {prompt_preview}… (no structured result)")
            }
        }
        "failed" => {
            let prompt_preview: String = prompt.chars().take(120).collect();
            let err_preview: String = error.unwrap_or("unknown").chars().take(120).collect();
            format!("[{subagent_type}] {prompt_preview}… failed: {err_preview}")
        }
        "cancelled" => {
            let prompt_preview: String = prompt.chars().take(120).collect();
            format!("[{subagent_type}] {prompt_preview}… (cancelled)")
        }
        _ => format!("[{subagent_type}] unknown status: {status}"),
    }
}

/// Build a summary string from a background job's state.
pub fn job_summary(
    tool_name: &str,
    output: Option<&Value>,
    error: Option<&str>,
    is_success: bool,
) -> String {
    if is_success {
        if let Some(o) = output {
            let out_str = serde_json::to_string(o).unwrap_or_default();
            let truncated: String = out_str.chars().take(200).collect();
            format!("[{tool_name}] completed: {truncated}")
        } else {
            format!("[{tool_name}] completed")
        }
    } else {
        let err_preview: String = error.unwrap_or("unknown").chars().take(120).collect();
        format!("[{tool_name}] failed: {err_preview}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn unique_conv() -> String {
        format!("tasks-test-{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn test_sub_agent_summary_completed() {
        let s = sub_agent_summary(
            "explore",
            "Find all TODO comments in the project",
            Some(&json!({"count": 5})),
            None,
            "completed",
        );
        assert!(s.contains("[explore]"));
        assert!(s.contains("Find all TODO"));
        assert!(s.contains("count"));
    }

    #[test]
    fn test_sub_agent_summary_failed() {
        let s = sub_agent_summary("implement", "Refactor the database layer", None, Some("Network timeout"), "failed");
        assert!(s.contains("[implement]"));
        assert!(s.contains("Network timeout"));
    }

    #[test]
    fn test_job_summary_completed() {
        let s = job_summary("llm_map", Some(&json!({"status": "success"})), None, true);
        assert!(s.contains("[llm_map]"));
        assert!(s.contains("success"));
    }

    #[test]
    fn test_job_summary_failed() {
        let s = job_summary("scraper", None, Some("Connection refused"), false);
        assert!(s.contains("[scraper]"));
        assert!(s.contains("Connection refused"));
    }
}
