//! Sub-agent tools — `spawn_sub_agent`, `sub_agent_status`, `cancel_sub_agent`.
//!
//! These tools enable the main agent to spawn async sub-agents that run
//! independently, check their status, and cancel them.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::sub_agents::manager::{SubAgentManager, DEFAULT_MAX_TURNS, HARD_MAX_TURNS};
use crate::sub_agents::types::{SubAgentSpec, SubAgentType};
use crate::tools::{Tool, ToolExecutionContext, check_scope_narrowing_invariant};
use serde::Deserialize;
use serde_json::{Value, json};

// ── SpawnSubAgentTool ─────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SpawnSubAgentArgs {
    prompt: String,
    #[serde(default)]
    subagent_type: Option<String>,
    #[serde(default)]
    delegated_scope: Vec<String>,
    #[serde(default)]
    kept_work: Vec<String>,
    #[serde(default = "default_max_turns")]
    max_turns: usize,
    #[serde(default)]
    use_worktree: bool,
}

fn default_max_turns() -> usize {
    DEFAULT_MAX_TURNS
}

pub struct SpawnSubAgentTool;

#[async_trait::async_trait]
impl Tool for SpawnSubAgentTool {
    fn name(&self) -> &'static str {
        "spawn_sub_agent"
    }

    fn description(&self) -> &'static str {
        "Spawn an async sub-agent to perform a task independently. \
         The sub-agent runs in the background and you can check its status \
         later with sub_agent_status. Use this for parallel work or tasks \
         that can be delegated. You MUST specify delegated_scope (which tools \
         the sub-agent may use) and kept_work (what output it will produce)."
    }

    fn display_name(&self) -> &'static str {
        "Spawn Sub-Agent"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Bot")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The task for the sub-agent to perform."
                },
                "subagentType": {
                    "type": "string",
                    "enum": ["explore", "codeReview", "implement", "analyze", "general"],
                    "description": "The type of sub-agent. 'explore' is read-only."
                },
                "delegatedScope": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tools/capabilities the sub-agent may access."
                },
                "keptWork": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Concrete outputs the sub-agent will produce."
                },
                "maxTurns": {
                    "type": "integer",
                    "description": "Maximum tool-using turns (default 5, max 10).",
                    "default": 5
                },
                "useWorktree": {
                    "type": "boolean",
                    "description": "Whether to use an isolated git worktree.",
                    "default": false
                }
            },
            "required": ["prompt", "subagentType"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: SpawnSubAgentArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::sub_agent(format!("Invalid arguments: {e}")))?;

        // Scope-narrowing invariant (LCM §3.2).
        check_scope_narrowing_invariant(
            &args.subagent_type,
            &args.delegated_scope,
            &args.kept_work,
            context,
        )?;

        let subagent_type = SubAgentType::from_str(
            args.subagent_type.as_deref().unwrap_or("general"),
        )
        .unwrap_or(SubAgentType::GeneralPurpose);

        let max_turns = args.max_turns.min(HARD_MAX_TURNS);

        let conversation_id = context
            .conversation_id
            .clone()
            .unwrap_or_else(|| "unknown".to_string());

        let agent_id = format!("agent_{}", uuid::Uuid::new_v4().simple());

        let spec = SubAgentSpec {
            agent_id: agent_id.clone(),
            parent_conversation_id: conversation_id,
            subagent_type,
            prompt: args.prompt,
            delegated_scope: args.delegated_scope,
            kept_work: args.kept_work,
            max_turns,
            use_worktree: args.use_worktree,
            model_id: context.model_id.clone(),
            parent_request_id: "tool-call".to_string(), // Will be set by caller
        };

        // Register the task in the process-wide registry.
        let _task = SubAgentManager::register(spec);

        // Note: The actual tokio::spawn of the runner happens at a higher
        // level (in the chat stream handler) where we have access to the
        // ToolCatalog, AppConfig, and Tauri window for events.
        // For now, we return the agent_id so the main agent can check status.

        Ok(json!({
            "ok": true,
            "agentId": agent_id,
            "status": "pending",
            "hint": "The sub-agent has been registered. Use sub_agent_status(agentId) to check progress, or sub_agent_status with wait=true to block until completion."
        }))
    }
}

// ── SubAgentStatusTool ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubAgentStatusArgs {
    agent_id: String,
    #[serde(default)]
    wait: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

pub struct SubAgentStatusTool;

#[async_trait::async_trait]
impl Tool for SubAgentStatusTool {
    fn name(&self) -> &'static str {
        "sub_agent_status"
    }

    fn description(&self) -> &'static str {
        "Check the status of an async sub-agent. Returns the current status, \
         progress, and result if completed. Set wait=true to block until \
         the sub-agent reaches a terminal state."
    }

    fn display_name(&self) -> &'static str {
        "Sub-Agent Status"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Activity")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agentId": {
                    "type": "string",
                    "description": "The ID of the sub-agent to check."
                },
                "wait": {
                    "type": "boolean",
                    "description": "If true, block until the sub-agent completes.",
                    "default": false
                },
                "timeoutMs": {
                    "type": "integer",
                    "description": "Maximum wait time in milliseconds (default 30000)."
                }
            },
            "required": ["agentId"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: SubAgentStatusArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::sub_agent(format!("Invalid arguments: {e}")))?;

        let conversation_id = context.conversation_id.as_deref();

        if args.wait {
            SubAgentManager::wait(&args.agent_id, args.timeout_ms, conversation_id).await
        } else {
            SubAgentManager::status(&args.agent_id, conversation_id)
        }
    }
}

// ── CancelSubAgentTool ────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelSubAgentArgs {
    agent_id: String,
}

pub struct CancelSubAgentTool;

#[async_trait::async_trait]
impl Tool for CancelSubAgentTool {
    fn name(&self) -> &'static str {
        "cancel_sub_agent"
    }

    fn description(&self) -> &'static str {
        "Cancel a running sub-agent. The sub-agent will be stopped and its \
         status will be set to 'cancelled'."
    }

    fn display_name(&self) -> &'static str {
        "Cancel Sub-Agent"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("StopCircle")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "agentId": {
                    "type": "string",
                    "description": "The ID of the sub-agent to cancel."
                }
            },
            "required": ["agentId"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: CancelSubAgentArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::sub_agent(format!("Invalid arguments: {e}")))?;

        let conversation_id = context.conversation_id.as_deref();
        SubAgentManager::cancel(&args.agent_id, conversation_id)
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::ToolExecutionContext;

    #[test]
    fn test_scope_narrowing_rejects_empty_kept_work_for_non_root() {
        let ctx = ToolExecutionContext {
            hop_index: Some(1),
            ..Default::default()
        };
        let result = check_scope_narrowing_invariant(
            &Some("implement".to_string()),
            &["filesystem".to_string()],
            &[],
            &ctx,
        );
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("kept_work"), "Should reject empty kept_work: {err}");
    }

    #[test]
    fn test_scope_narrowing_allows_root_agent() {
        let ctx = ToolExecutionContext {
            hop_index: Some(0),
            ..Default::default()
        };
        let result = check_scope_narrowing_invariant(
            &Some("implement".to_string()),
            &[],
            &[],
            &ctx,
        );
        assert!(result.is_ok(), "Root agent should be exempt");
    }

    #[test]
    fn test_scope_narrowing_allows_explore() {
        let ctx = ToolExecutionContext {
            hop_index: Some(2),
            ..Default::default()
        };
        let result = check_scope_narrowing_invariant(
            &Some("explore".to_string()),
            &[],
            &[],
            &ctx,
        );
        assert!(result.is_ok(), "Explore agent should be exempt");
    }

    #[test]
    fn test_spawn_sub_agent_args_deserialization() {
        let args = json!({
            "prompt": "Find all bugs",
            "subagentType": "explore",
            "delegatedScope": ["filesystem"],
            "keptWork": ["bug_list"],
            "maxTurns": 3
        });
        let parsed: SpawnSubAgentArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.prompt, "Find all bugs");
        assert_eq!(parsed.subagent_type, Some("explore".to_string()));
        assert_eq!(parsed.max_turns, 3);
    }

    #[test]
    fn test_defaults() {
        let args = json!({
            "prompt": "Do something",
            "subagentType": "general"
        });
        let parsed: SpawnSubAgentArgs = serde_json::from_value(args).unwrap();
        assert!(parsed.delegated_scope.is_empty());
        assert!(parsed.kept_work.is_empty());
        assert_eq!(parsed.max_turns, DEFAULT_MAX_TURNS);
        assert!(!parsed.use_worktree);
    }
}
