//! Sub-agent tool — `sub_agent`.
//!
//! A consolidated native tool that replaces the three separate tools
//! (`spawn_sub_agent`, `sub_agent_status`, `cancel_sub_agent`) with a
//! single `sub_agent` tool that dispatches based on an `action` parameter.
//!
//! ## Actions
//!
//! - **spawn** — register and launch an async sub-agent
//! - **status** — check the current state and/or result of a sub-agent
//! - **cancel** — cancel a running sub-agent

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::sub_agents::manager::{SubAgentManager, DEFAULT_MAX_TURNS, HARD_MAX_TURNS};
use crate::sub_agents::types::{SubAgentSpec, SubAgentType};
use crate::tools::{Tool, ToolExecutionContext, check_scope_narrowing_invariant};
use serde::Deserialize;
use serde_json::{Value, json};

// ── Argument structs ────────────────────────────────────────────────────────

/// Top-level arguments: the `action` discriminator plus action-specific fields.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SubAgentArgs {
    /// The operation to perform.
    action: String,

    // ── spawn fields ────────────────────────────────────────────────────
    #[serde(default)]
    prompt: Option<String>,
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

    // ── status / cancel fields ──────────────────────────────────────────
    #[serde(default)]
    agent_id: Option<String>,
    #[serde(default)]
    wait: bool,
    #[serde(default)]
    timeout_ms: Option<u64>,
}

fn default_max_turns() -> usize {
    DEFAULT_MAX_TURNS
}

// ── SubAgentTool ────────────────────────────────────────────────────────────

pub struct SubAgentTool;

#[async_trait::async_trait]
impl Tool for SubAgentTool {
    fn name(&self) -> &'static str {
        "sub_agent"
    }

    fn description(&self) -> &'static str {
        "Manage async sub-agents that perform tasks independently. \
         Use action 'spawn' to launch a new sub-agent, 'status' to check \
         its progress and results, or 'cancel' to stop a running sub-agent. \
         Spawned sub-agents run in the background while you continue working."
    }

    fn display_name(&self) -> &'static str {
        "Sub-Agent"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Bot")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "required": ["action"],
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["spawn", "status", "cancel"],
                    "description": "The sub-agent operation: 'spawn' to launch, 'status' to check progress, 'cancel' to stop."
                },
                "prompt": {
                    "type": "string",
                    "description": "[spawn] The task for the sub-agent to perform."
                },
                "subagentType": {
                    "type": "string",
                    "enum": ["explore", "codeReview", "implement", "analyze", "general", "memory"],
                    "description": "[spawn] The type of sub-agent. 'explore' is read-only, 'memory' is for background memory management."
                },
                "delegatedScope": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "[spawn] Tools/capabilities the sub-agent may access."
                },
                "keptWork": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "[spawn] Concrete outputs the sub-agent will produce."
                },
                "maxTurns": {
                    "type": "integer",
                    "description": "[spawn] Maximum tool-using turns (default 5, max 10).",
                    "default": 5
                },
                "useWorktree": {
                    "type": "boolean",
                    "description": "[spawn] Whether to use an isolated git worktree.",
                    "default": false
                },
                "agentId": {
                    "type": "string",
                    "description": "[status/cancel] The ID of the sub-agent."
                },
                "wait": {
                    "type": "boolean",
                    "description": "[status] If true, block until the sub-agent completes.",
                    "default": false
                },
                "timeoutMs": {
                    "type": "integer",
                    "description": "[status] Maximum wait time in milliseconds (default 30000)."
                }
            }
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: SubAgentArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::sub_agent(format!("Invalid arguments: {e}")))?;

        match args.action.as_str() {
            "spawn" => execute_spawn(&args, context).await,
            "status" => execute_status(&args, context).await,
            "cancel" => execute_cancel(&args, context).await,
            other => Err(AgentJaxError::sub_agent(format!(
                "Unknown action '{}'. Valid actions: spawn, status, cancel.",
                other
            ))),
        }
    }
}

// ── Action implementations ──────────────────────────────────────────────────

async fn execute_spawn(
    args: &SubAgentArgs,
    context: &ToolExecutionContext,
) -> AgentJaxResult<Value> {
    let prompt = args.prompt.as_deref().unwrap_or("").to_string();
    if prompt.is_empty() {
        return Err(AgentJaxError::sub_agent(
            "The 'prompt' field is required for action 'spawn'.".to_string(),
        ));
    }

    let subagent_type = SubAgentType::from_str(
        args.subagent_type.as_deref().unwrap_or("general"),
    )
    .unwrap_or(SubAgentType::GeneralPurpose);

    // Scope-narrowing invariant (LCM §3.2).
    check_scope_narrowing_invariant(
        &args.subagent_type,
        &args.delegated_scope,
        &args.kept_work,
        context,
    )?;

    let max_turns = args.max_turns.min(HARD_MAX_TURNS);

    let conversation_id = context
        .conversation_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let agent_id = format!("agent_{}", uuid::Uuid::new_v4().simple());

    let is_persistent = matches!(subagent_type, SubAgentType::Memory);

    let spec = SubAgentSpec {
        agent_id: agent_id.clone(),
        parent_conversation_id: conversation_id,
        subagent_type: subagent_type.clone(),
        prompt,
        delegated_scope: args.delegated_scope.clone(),
        kept_work: args.kept_work.clone(),
        max_turns,
        use_worktree: args.use_worktree,
        model_id: context.model_id.clone(),
        parent_request_id: "tool-call".to_string(),
        persistent: is_persistent,
    };

    // Register the task in the process-wide registry.
    let _task = SubAgentManager::register(spec);

    // Note: The actual tokio::spawn of the runner is wired in Phase 5
    // (chat stream handler), where AppConfig, ToolCatalog, and the Tauri
    // window for event forwarding are available.

    Ok(json!({
        "ok": true,
        "agentId": agent_id,
        "status": "pending",
        "subagentType": subagent_type.as_str(),
        "hint": "The sub-agent has been registered. Use sub_agent(action='status', agentId=...) to check progress, or action='status' with wait=true to block until completion."
    }))
}

async fn execute_status(
    args: &SubAgentArgs,
    context: &ToolExecutionContext,
) -> AgentJaxResult<Value> {
    let agent_id = args
        .agent_id
        .as_deref()
        .ok_or_else(|| {
            AgentJaxError::sub_agent(
                "The 'agentId' field is required for action 'status'.".to_string(),
            )
        })?;

    let conversation_id = context.conversation_id.as_deref();

    if args.wait {
        SubAgentManager::wait(agent_id, args.timeout_ms, conversation_id).await
    } else {
        SubAgentManager::status(agent_id, conversation_id)
    }
}

async fn execute_cancel(
    args: &SubAgentArgs,
    context: &ToolExecutionContext,
) -> AgentJaxResult<Value> {
    let agent_id = args
        .agent_id
        .as_deref()
        .ok_or_else(|| {
            AgentJaxError::sub_agent(
                "The 'agentId' field is required for action 'cancel'.".to_string(),
            )
        })?;

    let conversation_id = context.conversation_id.as_deref();
    SubAgentManager::cancel(agent_id, conversation_id)
}

// ── Tests ───────────────────────────────────────────────────────────────────

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
    fn test_scope_narrowing_allows_memory() {
        let ctx = ToolExecutionContext {
            hop_index: Some(2),
            ..Default::default()
        };
        let result = check_scope_narrowing_invariant(
            &Some("memory".to_string()),
            &[],
            &[],
            &ctx,
        );
        assert!(result.is_ok(), "Memory agent should be exempt from scope narrowing");
    }

    #[test]
    fn test_sub_agent_args_deserialization_spawn() {
        let args = json!({
            "action": "spawn",
            "prompt": "Find all bugs",
            "subagentType": "explore",
            "delegatedScope": ["filesystem"],
            "keptWork": ["bug_list"],
            "maxTurns": 3
        });
        let parsed: SubAgentArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "spawn");
        assert_eq!(parsed.prompt.unwrap(), "Find all bugs");
        assert_eq!(parsed.subagent_type.unwrap(), "explore");
        assert_eq!(parsed.max_turns, 3);
    }

    #[test]
    fn test_sub_agent_args_deserialization_status() {
        let args = json!({
            "action": "status",
            "agentId": "agent_abc123"
        });
        let parsed: SubAgentArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "status");
        assert_eq!(parsed.agent_id.unwrap(), "agent_abc123");
    }

    #[test]
    fn test_sub_agent_args_deserialization_cancel() {
        let args = json!({
            "action": "cancel",
            "agentId": "agent_xyz789"
        });
        let parsed: SubAgentArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "cancel");
        assert_eq!(parsed.agent_id.unwrap(), "agent_xyz789");
    }

    #[test]
    fn test_defaults() {
        let args = json!({
            "action": "spawn",
            "prompt": "Do something"
        });
        let parsed: SubAgentArgs = serde_json::from_value(args).unwrap();
        assert!(parsed.delegated_scope.is_empty());
        assert!(parsed.kept_work.is_empty());
        assert_eq!(parsed.max_turns, DEFAULT_MAX_TURNS);
        assert!(!parsed.use_worktree);
    }

    #[tokio::test]
    async fn test_execute_spawn_registers_task() {
        let ctx = ToolExecutionContext {
            conversation_id: Some("conv-test-spawn".to_string()),
            hop_index: Some(0),
            ..Default::default()
        };
        let args = json!({
            "action": "spawn",
            "prompt": "Echo hello",
            "subagentType": "explore",
            "delegatedScope": ["filesystem"],
            "keptWork": ["result"]
        });
        let tool = SubAgentTool;
        let result = tool.execute(&args, &ctx).await.unwrap();
        assert_eq!(result["ok"].as_bool().unwrap(), true);
        assert!(result["agentId"].as_str().unwrap().starts_with("agent_"));
        assert_eq!(result["status"].as_str().unwrap(), "pending");
    }

    #[test]
    fn test_unknown_action() {
        // We need a tokio runtime for async execute.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext::default();
            let args = json!({ "action": "nonexistent" });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("Unknown action"), "Got: {err}");
        });
    }

    #[test]
    fn test_spawn_missing_prompt() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext::default();
            let args = json!({ "action": "spawn" });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("'prompt'"), "Got: {err}");
        });
    }

    #[test]
    fn test_status_missing_agent_id() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext::default();
            let args = json!({ "action": "status" });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("'agentId'"), "Got: {err}");
        });
    }

    #[test]
    fn test_cancel_missing_agent_id() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext::default();
            let args = json!({ "action": "cancel" });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("'agentId'"), "Got: {err}");
        });
    }
}
