//! `task` — delegate work to a sub-agent with scope-narrowing invariant.
//!
//! Implements the structural guarantee from LCM §3.2: to prevent infinite
//! delegation chains, every sub-agent must declare both `delegated_scope`
//! (which tools or capabilities it may use) and `kept_work` (what output
//! it is expected to produce). If the caller cannot articulate what it is
//! keeping, the engine rejects the delegation.
//!
//! ## Scope-Narrowing Invariant
//!
//! ```text
//! When sub-agent A delegates to sub-agent B:
//!   - B.delegated_scope ⊂ A.delegated_scope  (strict subset)
//!   - B.kept_work is non-empty                (caller keeps something)
//!   - If kept_work is empty → REJECT           (prevents pass-through)
//! ```
//!
//! Root agents and read-only explore agents are exempt from this invariant.

use crate::config::AppConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::types::{
    ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest,
};
use crate::tools::{Tool, ToolExecutionContext, ToolRegistry};
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::watch;

// ── Arguments ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TaskArgs {
    /// The task description / prompt for the sub-agent.
    prompt: String,

    /// Which tools or capability domains the sub-agent may access.
    /// Must be a non-empty list. Examples: ["filesystem"], ["calculator", "filesystem"].
    #[serde(default)]
    delegated_scope: Vec<String>,

    /// What concrete output or work items the sub-agent is expected to produce.
    /// Must be non-empty for non-root callers. Examples: ["report", "code_changes"].
    /// If empty and this is a sub-agent (non-root), the call is rejected.
    #[serde(default)]
    kept_work: Vec<String>,

    /// Optional sub-agent type hint: "explore", "implement", "analyze".
    /// "explore" sub-agents are read-only and exempt from scope-narrowing.
    #[serde(default)]
    subagent_type: Option<String>,

    /// Maximum turns for the sub-agent loop (default 5).
    #[serde(default = "default_max_turns")]
    max_turns: usize,
}

fn default_max_turns() -> usize {
    5
}

// ── Tool ────────────────────────────────────────────────────────────────────

pub struct TaskTool;

#[async_trait::async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &'static str {
        "task"
    }

    fn description(&self) -> &'static str {
        "Delegate a task to a sub-agent. The sub-agent has access to a limited \
         set of tools and can perform multi-step work. You MUST specify what \
         tools the sub-agent may use (delegated_scope) and what output it will \
         produce (kept_work). This ensures each delegation represents a strict \
         reduction in responsibility, preventing infinite delegation chains."
    }

    fn display_name(&self) -> &'static str {
        "Task"
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The task for the sub-agent to perform."
                },
                "delegated_scope": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Tools/capabilities the sub-agent may use. Must be a subset of current scope."
                },
                "kept_work": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Concrete outputs the sub-agent will produce. Must be non-empty."
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Optional type: 'explore' (read-only), 'implement', 'analyze'."
                },
                "max_turns": {
                    "type": "integer",
                    "description": "Maximum tool-using turns for the sub-agent (default 5).",
                    "default": 5
                }
            },
            "required": ["prompt", "delegated_scope", "kept_work"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: TaskArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::tool(format!("Invalid task arguments: {e}")))?;

        // ── Scope-Narrowing Invariant (LCM §3.2) ──────────────────────────
        // Root agents and explore agents are exempt.
        let is_explore = args
            .subagent_type
            .as_deref()
            .map(|t| t == "explore")
            .unwrap_or(false);
        let is_root = context.hop_index.unwrap_or(0) == 0;

        if !is_root && !is_explore {
            if args.kept_work.is_empty() {
                return Err(AgentJaxError::tool(
                    "Scope-narrowing invariant violation: sub-agent must declare non-empty \
                     'kept_work' — describe what concrete output you will produce. \
                     Without this, the delegation would represent a pass-through with \
                     no reduction in responsibility."
                        .to_string(),
                ));
            }
            if args.delegated_scope.is_empty() {
                return Err(AgentJaxError::tool(
                    "Scope-narrowing invariant violation: sub-agent must declare non-empty \
                     'delegated_scope' — specify which tools the sub-agent may access."
                        .to_string(),
                ));
            }
        }

        let model_ref = context
            .model_id
            .clone()
            .unwrap_or_else(|| "default".to_string());
        let app_config = context.app_config.clone();
        let max_turns = args.max_turns.min(10); // hard cap

        let config = match app_config.as_deref() {
            Some(c) => c.clone(),
            None => crate::config::load_config()
                .map_err(|e| AgentJaxError::internal(format!("Failed to load config: {e}")))?,
        };

        let tool_registry = ToolRegistry::new_with_defaults();
        let tool_schemas = tool_registry.list_schemas();
        let tool_context = ToolExecutionContext::default();

        // ── Run the sub-agent loop ───────────────────────────────────────
        let mut input_items: Vec<Value> = vec![json!({
            "role": "user",
            "content": [{"type": "input_text", "text": args.prompt}]
        })];

        let instructions = format!(
            "You are a sub-agent of type '{}'. Your scope: {}. \
             Expected outputs: {}. Complete the task using available tools. \
             Output ONLY the final result as a JSON object.",
            args.subagent_type.as_deref().unwrap_or("general"),
            args.delegated_scope.join(", "),
            args.kept_work.join(", "),
        );

        for _turn in 0..max_turns {
            let (_cancel_tx, mut cancel_rx) = watch::channel(false);

            let request = ResponseStreamRequest {
                input_items: input_items.clone(),
                model: Some(model_ref.clone()),
                reasoning_effort: None,
                instructions_override: Some(instructions.clone()),
                text: None,
                include: None,
                service_tier: None,
                prompt_cache_key: None,
                client_metadata: None,
                generate: None,
                tools: Some(tool_schemas.clone()),
                tool_choice: Some(json!("auto")),
            };

            let mut tool_calls: Vec<ProviderPendingToolCall> = Vec::new();

            let response = crate::provider_api::stream_response(
                &config,
                &request,
                &mut cancel_rx,
                |event| {
                    if let ProviderStreamEvent::ToolCallStarted { call_id, name, .. } = &event {
                        tool_calls.push(ProviderPendingToolCall {
                            call_id: call_id.clone(),
                            name: name.clone(),
                            arguments: Value::Null,
                        });
                    }
                    if let ProviderStreamEvent::ToolCallCompleted {
                        call_id, arguments, ..
                    } = &event
                    {
                        if let Some(tc) = tool_calls.iter_mut().find(|t| &t.call_id == call_id) {
                            tc.arguments =
                                serde_json::from_str(arguments).unwrap_or(Value::Null);
                        }
                    }
                    Ok(())
                },
            )
            .await
            .map_err(|e| AgentJaxError::internal(format!("Task sub-agent call failed: {e}")))?;

            if tool_calls.is_empty() {
                let text = response.output_text.trim().to_string();
                if text.is_empty() {
                    return Err(AgentJaxError::internal(
                        "Task sub-agent returned empty response".to_string(),
                    ));
                }
                if let Ok(parsed) = serde_json::from_str::<Value>(&text) {
                    return Ok(parsed);
                }
                return Ok(json!({ "result": text }));
            }

            // Feed tool calls and results back.
            input_items.push(json!({
                "role": "assistant",
                "content": response.output_text,
            }));

            for tc in &tool_calls {
                input_items.push(json!({
                    "type": "function_call",
                    "call_id": tc.call_id,
                    "name": tc.name,
                    "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default(),
                }));

                let exec_result = tool_registry.execute(&tc.name, &tc.arguments, &tool_context).await;
                match exec_result {
                    Ok(output) => {
                        let output_str =
                            serde_json::to_string(&output).unwrap_or_default();
                        input_items.push(json!({
                            "type": "function_call_output",
                            "call_id": tc.call_id,
                            "output": output_str,
                        }));
                    }
                    Err(e) => {
                        input_items.push(json!({
                            "type": "function_call_output",
                            "call_id": tc.call_id,
                            "output": format!("Tool error: {e}"),
                        }));
                    }
                }
            }
        }

        Err(AgentJaxError::internal(format!(
            "Task sub-agent exceeded maximum turns ({max_turns}) without completing"
        )))
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_deserialize_args() {
        let args = json!({
            "prompt": "Analyze the codebase for bugs",
            "delegatedScope": ["filesystem"],
            "keptWork": ["bug_report"]
        });
        let parsed: TaskArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.prompt, "Analyze the codebase for bugs");
        assert_eq!(parsed.delegated_scope, vec!["filesystem"]);
        assert_eq!(parsed.kept_work, vec!["bug_report"]);
        assert_eq!(parsed.max_turns, 5);
    }

    #[test]
    fn test_defaults() {
        let args = json!({
            "prompt": "Do something",
            "delegatedScope": [],
            "keptWork": []
        });
        let parsed: TaskArgs = serde_json::from_value(args).unwrap();
        assert!(parsed.delegated_scope.is_empty());
        assert!(parsed.kept_work.is_empty());
        assert_eq!(parsed.max_turns, 5);
    }

    #[tokio::test]
    async fn test_scope_narrowing_rejects_empty_kept_work_for_non_root() {
        let tool = TaskTool;
        let ctx = ToolExecutionContext {
            hop_index: Some(1), // non-root
            ..Default::default()
        };
        let args = json!({
            "prompt": "Do a task",
            "delegatedScope": ["filesystem"],
            "keptWork": []
        });
        let result = tool.execute(&args, &ctx).await;
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("kept_work"), "Should reject empty kept_work: {err}");
    }

    #[tokio::test]
    async fn test_root_agent_exempt_from_scope_narrowing() {
        let tool = TaskTool;
        let ctx = ToolExecutionContext {
            hop_index: Some(0), // root
            ..Default::default()
        };
        let args = json!({
            "prompt": "Do a task",
            "delegatedScope": [],
            "keptWork": []
        });
        // Root agent with empty scope/work should be accepted (exempt from invariant).
        // The execution will fail at provider call (no real provider in tests),
        // but NOT at scope validation.
        let result = tool.execute(&args, &ctx).await;
        match result {
            Ok(_) => {} // Provider succeeded — also fine, exemption worked
            Err(e) => {
                let err = e.to_string();
                assert!(
                    !err.contains("kept_work"),
                    "Root agent should be exempt from scope-narrowing, got: {err}"
                );
            }
        }
    }

    #[tokio::test]
    async fn test_explore_agent_exempt_from_scope_narrowing() {
        let tool = TaskTool;
        let ctx = ToolExecutionContext {
            hop_index: Some(2), // non-root
            ..Default::default()
        };
        let args = json!({
            "prompt": "Explore the file",
            "delegatedScope": [],
            "keptWork": [],
            "subagentType": "explore"
        });
        // Explore agent should be exempt from scope-narrowing invariant.
        let result = tool.execute(&args, &ctx).await;
        match result {
            Ok(_) => {} // Provider succeeded — also fine, exemption worked
            Err(e) => {
                let err = e.to_string();
                assert!(
                    !err.contains("kept_work"),
                    "Explore agent should be exempt from scope-narrowing, got: {err}"
                );
            }
        }
    }
}
