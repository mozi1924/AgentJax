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
use std::sync::Arc;

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

    // ── batch fields ───────────────────────────────────────────────────
    #[serde(default)]
    input_path: Option<String>,
    #[serde(default)]
    output_path: Option<String>,
}

    #[test]
    fn test_batch_args_deserialization() {
        let args = json!({
            "action": "batch",
            "prompt": "Analyze: {input}",
            "inputPath": "items.jsonl",
            "outputPath": "results.jsonl",
            "maxTurns": 8
        });
        let parsed: SubAgentArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "batch");
        assert_eq!(parsed.prompt.unwrap(), "Analyze: {input}");
        assert_eq!(parsed.input_path.unwrap(), "items.jsonl");
        assert_eq!(parsed.output_path.unwrap(), "results.jsonl");
        assert_eq!(parsed.max_turns, 8);
    }

    #[test]
    fn test_batch_args_defaults_work_with_spawn_fields() {
        // Batch reuses spawn fields as batch defaults.
        let args = json!({
            "action": "batch",
            "prompt": "Classify: {input}",
            "inputPath": "data.jsonl",
            "subagentType": "explore",
            "keptWork": ["classification"]
        });
        let parsed: SubAgentArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.action, "batch");
        assert_eq!(parsed.subagent_type.unwrap(), "explore");
        assert_eq!(parsed.kept_work, vec!["classification"]);
        assert_eq!(parsed.max_turns, DEFAULT_MAX_TURNS);
    }

    #[test]
    fn test_batch_missing_input_path() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext::default();
            let args = json!({ "action": "batch", "prompt": "Analyze: {input}" });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("inputPath"), "Got: {err}");
        });
    }

    #[test]
    fn test_batch_missing_prompt() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext::default();
            let args = json!({ "action": "batch", "inputPath": "items.jsonl" });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("prompt"), "Got: {err}");
        });
    }

    #[test]
    fn test_batch_input_file_not_found() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let ctx = ToolExecutionContext {
                conversation_id: Some("conv-test-batch".to_string()),
                ..Default::default()
            };
            let args = json!({
                "action": "batch",
                "prompt": "Analyze: {input}",
                "inputPath": "nonexistent_file.jsonl"
            });
            let tool = SubAgentTool;
            let result = tool.execute(&args, &ctx).await;
            assert!(result.is_err());
            let err = result.unwrap_err().to_string();
            assert!(err.contains("not found"), "Got: {err}");
        });
    }

    #[test]
    fn test_batch_registers_multiple_agents() {
        // Create a temp JSONL file in the conversation workspace and test batch registration.
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async {
            let conv_id = format!("conv_batch_test_{}", uuid::Uuid::new_v4());
            // Create the conversation workspace directory
            let workspace_dir = crate::conversation_store::conversation_workspace_path(&conv_id).unwrap();
            std::fs::create_dir_all(&workspace_dir).unwrap();
            let input_path = workspace_dir.join("items.jsonl");
            std::fs::write(&input_path, r#"{"item": 1}
{"item": 2}
{"item": 3}
"#).unwrap();

            let ctx = ToolExecutionContext {
                conversation_id: Some(conv_id.clone()),
                ..Default::default()
            };
            let args = json!({
                "action": "batch",
                "prompt": "Analyze: {input}",
                "inputPath": "items.jsonl",
                "keptWork": ["result"]
            });
            let tool = SubAgentTool;
            // In test env, sub-agents will fail (no provider configured).
            // The batch coordinator should still complete without error,
            // returning the correct count and status.
            let result = tool.execute(&args, &ctx).await.unwrap();
            assert!(result["batchMode"].as_bool().unwrap(), "Should be batch mode");
            assert_eq!(result["totalItems"].as_i64().unwrap(), 3);
            assert_eq!(result["completedItems"].as_i64().unwrap(), 0);
            assert_eq!(result["failedItems"].as_i64().unwrap(), 3);
            assert!(result["status"].as_str().unwrap() == "partial", 
                "Status should be partial, got: {:?}", result["status"].as_str());
            // Cleanup: remove the workspace directory
            let _ = std::fs::remove_dir_all(&workspace_dir);
        });
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
         Use action 'spawn' for a single sub-agent, 'batch' to spawn one \
         per JSONL item with concurrency and retry, 'status' to check \
         progress, or 'cancel' to stop. Spawned sub-agents run in the \
         background while you continue working."
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
                    "enum": ["spawn", "status", "cancel", "batch"],
                    "description": "The sub-agent operation: 'spawn' to launch, 'status' to check progress, 'cancel' to stop, 'batch' to spawn one sub-agent per JSONL item."
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
                },
                "inputPath": {
                    "type": "string",
                    "description": "[batch] Path to the JSONL input file (one item per line). Each item replaces {input} in the prompt."
                },
                "outputPath": {
                    "type": "string",
                    "description": "[batch] Path to write the JSONL output file. If omitted, results must be collected manually."
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
            "batch" => execute_batch(&args, context).await,
            other => Err(AgentJaxError::sub_agent(format!(
                "Unknown action '{}'. Valid actions: spawn, status, cancel, batch.",
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
        max_retries: 0,
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

// ── Batch implementation ──────────────────────────────────────────────────

/// Execute a batch spawn: read a JSONL file, spawn one sub-agent per item,
/// wait for results with concurrency control and retry, and write the output file.
async fn execute_batch(
    args: &SubAgentArgs,
    context: &ToolExecutionContext,
) -> AgentJaxResult<Value> {
    let input_path = args.input_path.as_deref().ok_or_else(|| {
        AgentJaxError::sub_agent(
            "The 'inputPath' field is required for action 'batch'.".to_string(),
        )
    })?;

    let prompt_template = args.prompt.as_deref().ok_or_else(|| {
        AgentJaxError::sub_agent(
            "The 'prompt' field is required for action 'batch'.".to_string(),
        )
    })?;

    // Resolve path relative to the conversation workspace.
    let workspace_dir = context
        .conversation_id
        .as_deref()
        .and_then(|id| crate::conversation_store::conversation_workspace_path(id).ok())
        .unwrap_or_else(|| std::path::PathBuf::from("."));
    let full_path = workspace_dir.join(input_path);

    if !full_path.exists() {
        return Err(AgentJaxError::not_found(format!(
            "Batch input file not found: {}",
            full_path.display()
        )));
    }

    let input_content = std::fs::read_to_string(&full_path)
        .map_err(|e| AgentJaxError::internal(format!("Failed to read batch input file: {e}")))?;

    let items: Vec<String> = input_content
        .lines()
        .filter(|l| !l.trim().is_empty())
        .map(|s| s.to_string())
        .collect();

    if items.is_empty() {
        return Err(AgentJaxError::sub_agent("Batch input file is empty".to_string()));
    }

    let conversation_id = context
        .conversation_id
        .clone()
        .unwrap_or_else(|| "unknown".to_string());

    let subagent_type = SubAgentType::from_str(
        args.subagent_type.as_deref().unwrap_or("general"),
    )
    .unwrap_or(SubAgentType::GeneralPurpose);

    let max_turns = args.max_turns.min(HARD_MAX_TURNS);
    let concurrency = std::cmp::max(1, 16); // default concurrency cap
    let max_retries: u32 = 2u32; // default retries

    // Create shared state for concurrent execution.

    let total = items.len();
    let semaphore = Arc::new(tokio::sync::Semaphore::new(concurrency));
    let results: Arc<std::sync::Mutex<Vec<(usize, String, Option<AgentJaxError>)>>> =
        Arc::new(std::sync::Mutex::new(Vec::with_capacity(total)));
    let completed = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let failed = Arc::new(std::sync::atomic::AtomicUsize::new(0));

    // Prepare catalog and config for sub-agent spawning.
    // We create a minimal catalog (native + plugin tools, no MCP) for batch items,
    // since we don't have access to the full Tauri MCP manager from within a tool.
    let batch_config = context
        .app_config
        .clone()
        .unwrap_or_else(|| Arc::new(crate::config::AppConfig::default()));
    let batch_catalog = Arc::new(crate::tools::ToolCatalog::new_with_home_plugins(
        Arc::new(crate::mcp::McpManager::new()),
        &batch_config,
    ));
    let (_batch_event_tx, _batch_event_rx) = tokio::sync::mpsc::unbounded_channel();
    let batch_conv_id = conversation_id.clone();

    let mut handles = Vec::with_capacity(total);

    for (i, item) in items.iter().enumerate() {
        let item_text = item.clone();
        let prompt = prompt_template.replace("{input}", &item_text);
        let sem = semaphore.clone();
        let res = results.clone();
        let comp = completed.clone();
        let fail = failed.clone();
        let cfg = batch_config.clone();
        let catalog = batch_catalog.clone();
        let conv = batch_conv_id.clone();
        let agent_type = subagent_type.clone();
        let event_tx = _batch_event_tx.clone();
        let output_path = args.output_path.clone();
        let scope = args.delegated_scope.clone();
        let kept = args.kept_work.clone();
        let model = context.model_id.clone();

        handles.push(tokio::spawn(async move {
            let _permit = sem.acquire().await;
            let mut last_error: Option<AgentJaxError> = None;
            let retries = max_retries;

            for attempt in 0..=retries {
                if attempt > 0 {
                    tokio::time::sleep(std::time::Duration::from_millis(500 * attempt as u64)).await;
                }

                let agent_id = format!("batch_{}", uuid::Uuid::new_v4().simple());
                let spec = crate::sub_agents::types::SubAgentSpec {
                    agent_id: agent_id.clone(),
                    parent_conversation_id: conv.clone(),
                    subagent_type: agent_type.clone(),
                    prompt: prompt.clone(),
                    delegated_scope: scope.clone(),
                    kept_work: kept.clone(),
                    max_turns,
                    max_retries: 0, // internal retries handled here, not in the agent
                    use_worktree: false,
                    model_id: model.clone(),
                    parent_request_id: "batch".to_string(),
                    persistent: false,
                };

                let task = SubAgentManager::register(spec.clone());
                let handle = tokio::spawn(crate::sub_agents::runner::run_sub_agent(
                    task.clone(),
                    spec,
                    cfg.clone(),
                    catalog.clone(),
                    event_tx.clone(),
                ));
                SubAgentManager::mark_running(&task, handle);

                // Wait for the sub-agent to complete.
                match crate::sub_agents::manager::SubAgentManager::wait(
                    &agent_id,
                    Some(300_000),
                    None,
                )
                .await
                {
                    Ok(status_json) => {
                        let agent_status = status_json["agent"]["status"]
                            .as_str()
                            .unwrap_or("failed");
                        if agent_status == "completed" {
                            let result_text = status_json["agent"]["result"]
                                .to_string();
                            comp.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                            let mut lock = res.lock().unwrap();
                            lock.push((i, result_text, None));
                            return;
                        }
                        last_error = Some(AgentJaxError::sub_agent(format!(
                            "Item {} failed with status '{}'",
                            i, agent_status
                        )));
                    }
                    Err(e) => {
                        last_error = Some(e);
                    }
                }
            }

            fail.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let mut lock = res.lock().unwrap();
            lock.push((
                i,
                String::new(),
                Some(last_error.unwrap_or_else(|| {
                    AgentJaxError::sub_agent(format!("Item {} exhausted retries", i))
                })),
            ));
        }));
    }

    // Wait for all items to complete.
    for handle in handles {
        let _ = handle.await;
    }

    // Sort results by original item index.
    let mut sorted_results = {
        let mut lock = results.lock().unwrap();
        lock.sort_by_key(|(i, _, _)| *i);
        lock.clone()
    };

    let completed_count = completed.load(std::sync::atomic::Ordering::SeqCst);
    let failed_count = failed.load(std::sync::atomic::Ordering::SeqCst);

    // Write output file if output_path is specified.
    if let Some(output) = &args.output_path {
        let out_path = workspace_dir.join(output);
        let mut out_lines = Vec::with_capacity(sorted_results.len());

        for (_i, result_text, error) in &sorted_results {
            if let Some(err) = error {
                out_lines.push(serde_json::to_string(&json!({
                    "error": err.to_string(),
                    "status": "failed"
                })).unwrap_or_default());
            } else {
                // Try to parse the result as JSON; if not possible, wrap as raw text.
                let line = serde_json::from_str::<Value>(result_text)
                    .unwrap_or_else(|_| json!({ "result": result_text, "status": "success" }));
                out_lines.push(serde_json::to_string(&line).unwrap_or_default());
            }
        }

        if let Some(parent) = out_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Err(e) = std::fs::write(&out_path, out_lines.join("\n")) {
            log::warn!("Failed to write batch output to {}: {e}", out_path.display());
        }
    }

    let summary = if failed_count == 0 {
        format!(
            "All {total} items processed successfully. {}",
            if args.output_path.is_some() {
                format!("Output written to {}", args.output_path.as_deref().unwrap())
            } else {
                String::new()
            }
        )
    } else {
        format!(
            "Processed {total} items: {completed_count} succeeded, {failed_count} failed.",
            total = total,
            completed_count = completed_count,
            failed_count = failed_count,
        )
    };

    Ok(json!({
        "ok": true,
        "batchMode": true,
        "totalItems": total,
        "completedItems": completed_count,
        "failedItems": failed_count,
        "agentIds": sorted_results.iter().map(|(_, result, error)| json!({"result": result, "error": error.as_ref().map(|e| e.to_string())})).collect::<Vec<_>>(),
        "status": if failed_count == 0 { "completed" } else { "partial" },
        "summary": summary,
    }))
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
