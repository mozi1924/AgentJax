//! Sub-agent runner — executes a sub-agent task asynchronously via `tokio::spawn`.
//!
//! The runner initializes an isolated LCM engine, optionally creates a git
//! worktree, builds a `ChatRequest` from the `SubAgentSpec`, and calls
//! `AgentRuntime::run_turn` to execute the full multi-hop agent loop.
//!
//! Progress events are sent through an mpsc channel so they can be forwarded
//! to the frontend via Tauri events.

use crate::commands::chat::ChatRequest;
use crate::config::{AgentConfig, AppConfig};
use crate::provider_api::types::ProviderStreamEvent;
use crate::runtime::agent_context::InMemoryContext;
use crate::sub_agents::events::SubAgentEvent;
use crate::sub_agents::manager::{HARD_MAX_TURNS, SubAgentManager, SubAgentTask};
use crate::sub_agents::types::SubAgentSpec;
use crate::sub_agents::worktree::Worktree;
use crate::tools::ToolCatalog;
use serde_json::{Value, json};
use std::sync::Arc;
use tokio::sync::mpsc;

// ── Runner ────────────────────────────────────────────────────────────────────

/// Run a sub-agent task to completion.
///
/// This function is called inside a `tokio::spawn` block. It:
/// 1. Creates an in-memory context for the sub-agent
/// 2. Optionally creates a git worktree
/// 3. Builds a ChatRequest from the spec
/// 4. Calls AgentRuntime::run_turn
/// 5. Stores the result in the SubAgentTask
/// 6. Emits progress events through the provided channel
pub async fn run_sub_agent(
    task: Arc<SubAgentTask>,
    spec: SubAgentSpec,
    app_config: Arc<AppConfig>,
    agent_config: Arc<AgentConfig>,
    tools_catalog: Arc<ToolCatalog>,
    event_tx: mpsc::UnboundedSender<SubAgentEvent>,
) {
    let agent_id = spec.agent_id.clone();
    let max_turns = spec.max_turns.min(HARD_MAX_TURNS);

    // Emit spawned event.
    let _ = event_tx.send(SubAgentEvent::Spawned {
        agent_id: agent_id.clone(),
        subagent_type: spec.subagent_type.as_str().to_string(),
        parent_request_id: spec.parent_request_id.clone(),
    });

    // ── Create isolated in-memory context ─────────────────────────────────
    // Sub-agents use a lightweight in-memory message buffer. No SQLite,
    // no compaction, no disk I/O — automatically cleaned up on drop.
    let sub_conv_id = format!(
        "{}/sub-agent/{}/{}",
        spec.parent_conversation_id,
        spec.subagent_type.as_str(),
        agent_id
    );
    let agent_ctx = InMemoryContext::new();

    // Emit started event.
    let _ = event_tx.send(SubAgentEvent::Started {
        agent_id: agent_id.clone(),
    });

    // ── Optionally create worktree ────────────────────────────────────────
    let _worktree: Option<Worktree> = if spec.use_worktree {
        match Worktree::create(&agent_id, &spec.parent_conversation_id) {
            Ok(wt) => {
                log::info!(
                    "Sub-agent {}: created worktree at {}",
                    agent_id,
                    wt.path.display()
                );
                Some(wt)
            }
            Err(e) => {
                log::warn!("Sub-agent {}: worktree creation failed: {e}", agent_id);
                None
            }
        }
    } else {
        None
    };

    // ── Build ChatRequest ─────────────────────────────────────────────────
    let model_id = spec.model_id.clone();
    let sub_req = ChatRequest {
        input: build_sub_agent_instructions(&spec),
        conversation_id: Some(sub_conv_id.clone()),
        model: model_id.or_else(|| {
            if agent_config.utility_small_model.is_empty() {
                Some(agent_config.default_model.clone())
            } else {
                Some(agent_config.utility_small_model.clone())
            }
        }),
        reasoning: None,
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        request_id: Some(format!("sub-agent-{}", agent_id)),
        agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
        temperature: None,
        top_p: None,
        presence_penalty: None,
        frequency_penalty: None,
        max_tokens: None,
        max_completion_tokens: None,
    };

    // ── Run the agent loop ────────────────────────────────────────────────
    // Wire the task's cancel signal so run_turn receives actual cancellations.
    let (merged_cancel_tx, mut merged_cancel_rx) = tokio::sync::watch::channel(false);

    // Forward task cancellation to merged_cancel_rx.
    let mut task_cancel_rx = task.cancel_tx.subscribe();
    let cancel_fwd_agent_id = agent_id.clone();
    let cancel_fwd_event_tx = event_tx.clone();
    tokio::spawn(async move {
        loop {
            let changed = task_cancel_rx.changed().await;
            if changed.is_err() {
                break;
            }
            if *task_cancel_rx.borrow() {
                let _ = merged_cancel_tx.send(true);
                let _ = cancel_fwd_event_tx.send(SubAgentEvent::Cancelled {
                    agent_id: cancel_fwd_agent_id.clone(),
                    reason: "Sub-agent cancelled by user".to_string(),
                });
                break;
            }
        }
    });

    // Clone values that need to be used both inside and outside the closure.
    let closure_event_tx = event_tx.clone();
    let closure_agent_id = agent_id.clone();
    let run_event_tx = event_tx.clone(); // For the tool execution context
    let result = crate::runtime::AgentRuntime::run_turn(
        &app_config,
        &agent_config,
        &agent_id,
        &sub_req,
        &sub_conv_id,
        crate::conversation_store_utils::now_unix_ms(),
        Vec::new(), // No prior context for sub-agents
        None,       // No recovery note
        &tools_catalog,
        &agent_ctx,
        &mut merged_cancel_rx,
        Some(run_event_tx),
        Vec::new(), // street_items — sub-agents don't receive Street notifications
        false,      // is_auto_resume
        move |event| {
            // Map provider events to sub-agent events for progress tracking.
            match &event {
                ProviderStreamEvent::HopAssistantText { text, phase, .. }
                    if phase.is_none_or(|p| p.as_str() != "commentary") =>
                {
                    let _ = closure_event_tx.send(SubAgentEvent::Progress {
                        agent_id: closure_agent_id.clone(),
                        text: text.chars().take(200).collect(),
                        turns_completed: 0,
                        turns_remaining: max_turns,
                    });
                }
                ProviderStreamEvent::ToolCallStarted { call_id, name, .. } => {
                    let _ = closure_event_tx.send(SubAgentEvent::ToolCallStarted {
                        agent_id: closure_agent_id.clone(),
                        call_id: call_id.clone(),
                        tool_name: name.clone(),
                    });
                }
                ProviderStreamEvent::ToolCallExecuted {
                    call_id,
                    name,
                    is_success,
                    ..
                } => {
                    let _ = closure_event_tx.send(SubAgentEvent::ToolCallCompleted {
                        agent_id: closure_agent_id.clone(),
                        call_id: call_id.clone(),
                        tool_name: name.clone(),
                        tool_status: if *is_success {
                            "done".to_string()
                        } else {
                            "failed".to_string()
                        },
                    });
                }
                _ => {}
            }
            Ok(())
        },
    )
    .await;

    match result {
        Ok((response, _timeline)) => {
            // Try to parse the output as JSON; fall back to wrapping in a result object.
            let parsed_result =
                if let Ok(val) = serde_json::from_str::<Value>(&response.output_text) {
                    val
                } else {
                    json!({ "result": response.output_text })
                };

            let _ = event_tx.send(SubAgentEvent::Completed {
                agent_id: agent_id.clone(),
                result: parsed_result.clone(),
                duration_ms: 0, // Will be set by complete()
            });

            SubAgentManager::complete(&task, parsed_result);
        }
        Err(e) => {
            let err_msg = e.to_string();
            let _ = event_tx.send(SubAgentEvent::Failed {
                agent_id: agent_id.clone(),
                error: err_msg.clone(),
                duration_ms: 0,
            });

            SubAgentManager::fail(&task, err_msg);
        }
    }

    // ── Cleanup ───────────────────────────────────────────────────────────
    if let Some(wt) = _worktree
        && let Err(e) = wt.cleanup()
    {
        log::warn!("Sub-agent {}: worktree cleanup failed: {e}", agent_id);
    }

    log::info!("Sub-agent {}: finished", agent_id);
}

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Build the system instructions and user prompt for a sub-agent.
fn build_sub_agent_instructions(spec: &SubAgentSpec) -> String {
    let scope = if spec.delegated_scope.is_empty() {
        "full tool access".to_string()
    } else {
        spec.delegated_scope.join(", ")
    };

    let outputs = if spec.kept_work.is_empty() {
        "complete the assigned task".to_string()
    } else {
        spec.kept_work.join(", ")
    };

    format!(
        "You are a sub-agent of type '{}'.\n\n\
         Your scope: {}\n\
         Expected outputs: {}\n\
         Maximum turns: {}\n\n\
         Complete the following task using the available tools. \
         Output ONLY the final result. If the result is structured data, \
         format it as a JSON object.\n\n\
         Task:\n{}",
        spec.subagent_type.as_str(),
        scope,
        outputs,
        spec.max_turns,
        spec.prompt,
    )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sub_agents::types::SubAgentType;

    #[test]
    fn test_build_instructions_includes_scope_and_prompt() {
        let spec = SubAgentSpec {
            agent_id: "test".to_string(),
            parent_conversation_id: "conv".to_string(),
            subagent_type: SubAgentType::Explore,
            prompt: "Find all .rs files".to_string(),
            delegated_scope: vec!["filesystem".to_string()],
            kept_work: vec!["file_list".to_string()],
            max_turns: 3,
            max_retries: 0,
            use_worktree: false,
            model_id: None,
            parent_request_id: "req".to_string(),
            persistent: false,
        };

        let instructions = build_sub_agent_instructions(&spec);
        assert!(instructions.contains("explore"));
        assert!(instructions.contains("filesystem"));
        assert!(instructions.contains("file_list"));
        assert!(instructions.contains("Find all .rs files"));
    }

    #[test]
    fn test_build_instructions_empty_scope_defaults() {
        let spec = SubAgentSpec {
            agent_id: "test".to_string(),
            parent_conversation_id: "conv".to_string(),
            subagent_type: SubAgentType::GeneralPurpose,
            prompt: "Do something".to_string(),
            delegated_scope: vec![],
            kept_work: vec![],
            max_turns: 5,
            max_retries: 0,
            use_worktree: false,
            model_id: None,
            parent_request_id: "req".to_string(),
            persistent: false,
        };

        let instructions = build_sub_agent_instructions(&spec);
        assert!(instructions.contains("full tool access"));
        assert!(instructions.contains("complete the assigned task"));
    }
}
