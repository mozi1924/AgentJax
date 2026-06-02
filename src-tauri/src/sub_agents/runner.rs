//! Sub-agent runner — executes a sub-agent task asynchronously via `tokio::spawn`.
//!
//! The runner initializes an isolated LCM engine, optionally creates a git
//! worktree, builds a `ChatRequest` from the `SubAgentSpec`, and calls
//! `AgentRuntime::run_turn` to execute the full multi-hop agent loop.
//!
//! Progress events are sent through an mpsc channel so they can be forwarded
//! to the frontend via Tauri events.

use crate::commands::chat::ChatRequest;
use crate::config::AppConfig;
use crate::provider_api::types::ProviderStreamEvent;
use crate::sub_agents::events::SubAgentEvent;
use crate::sub_agents::lcm_context::SubAgentLcmContext;
use crate::sub_agents::manager::{SubAgentManager, SubAgentTask, HARD_MAX_TURNS};
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
/// 1. Creates an isolated LCM engine for the sub-agent
/// 2. Optionally creates a git worktree
/// 3. Builds a ChatRequest from the spec
/// 4. Calls AgentRuntime::run_turn
/// 5. Stores the result in the SubAgentTask
/// 6. Emits progress events through the provided channel
pub async fn run_sub_agent(
    task: Arc<SubAgentTask>,
    spec: SubAgentSpec,
    app_config: Arc<AppConfig>,
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

    // ── Create isolated LCM ───────────────────────────────────────────────
    let lcm_config = app_config.lcm.clone();
    let sub_lcm = match SubAgentLcmContext::create(
        &spec.parent_conversation_id,
        &agent_id,
        &lcm_config,
    ) {
        Ok(ctx) => ctx,
        Err(e) => {
            let _ = event_tx.send(SubAgentEvent::Failed {
                agent_id: agent_id.clone(),
                error: format!("Failed to create LCM context: {e}"),
                duration_ms: 0,
            });
            SubAgentManager::fail(&task, format!("LCM initialization failed: {e}"));
            return;
        }
    };

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
        conversation_id: Some(sub_lcm.conversation_id.clone()),
        model: model_id.or_else(|| {
            if app_config.utility_small_model.is_empty() {
                Some(app_config.default_model.clone())
            } else {
                Some(app_config.utility_small_model.clone())
            }
        }),
        reasoning_effort: None,
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        request_id: Some(format!("sub-agent-{}", agent_id)),
    };

    // ── Run the agent loop ────────────────────────────────────────────────
    let (_cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
    // Merge with task's cancel signal.
    let mut task_cancel_rx = task.cancel_tx.subscribe();
    let mut merged_cancel_rx = tokio::sync::watch::channel(false).1;

    // We need to run the main loop with cancellation from either source.
    // Use a simple approach: poll both cancel sources.
    let cancel_handle = {
        let agent_id = agent_id.clone();
        let event_tx = event_tx.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    _ = task_cancel_rx.changed() => {
                        if *task_cancel_rx.borrow() {
                            let _ = event_tx.send(SubAgentEvent::Cancelled {
                                agent_id: agent_id.clone(),
                                reason: "Sub-agent cancelled by user".to_string(),
                            });
                            break;
                        }
                    }
                    _ = cancel_rx.changed() => {
                        break;
                    }
                }
            }
        })
    };

    // Clone values that need to be used both inside and outside the closure.
    let closure_event_tx = event_tx.clone();
    let closure_agent_id = agent_id.clone();
    let run_event_tx = event_tx.clone(); // For the tool execution context
    let result = crate::runtime::AgentRuntime::run_turn(
        &app_config,
        &sub_req,
        &sub_lcm.conversation_id,
        crate::conversation_store_utils::now_unix_ms(),
        Vec::new(), // No prior context for sub-agents
        None,       // No recovery note
        &tools_catalog,
        &sub_lcm.engine,
        &mut merged_cancel_rx,
        Some(run_event_tx),
        Vec::new(), // street_items — sub-agents don't receive Street notifications
        move |event| {
            // Map provider events to sub-agent events for progress tracking.
            match &event {
                ProviderStreamEvent::HopAssistantText { text, phase, .. } => {
                    if phase.map_or(true, |p| p.as_str() != "commentary") {
                        let _ = closure_event_tx.send(SubAgentEvent::Progress {
                            agent_id: closure_agent_id.clone(),
                            text: text.chars().take(200).collect(),
                            turns_completed: 0,
                            turns_remaining: max_turns,
                        });
                    }
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

    // Cancel the cancel watcher.
    cancel_handle.abort();

    match result {
        Ok((response, _timeline)) => {
            // Try to parse the output as JSON; fall back to wrapping in a result object.
            let parsed_result = if let Ok(val) =
                serde_json::from_str::<Value>(&response.output_text)
            {
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
    if let Some(wt) = _worktree {
        if let Err(e) = wt.cleanup() {
            log::warn!("Sub-agent {}: worktree cleanup failed: {e}", agent_id);
        }
    }

    log::info!("Sub-agent {}: finished", agent_id);
}

// ── Memory Agent Runner ───────────────────────────────────────────────────────

/// Run the persistent background memory sub-agent.
///
/// Unlike `run_sub_agent`, this is event-driven: it waits for signals
/// (TurnCompleted, Terminate) on a watch channel, evaluates conversation
/// context, and writes memories directly via MemoryStore.
pub async fn run_memory_agent(
    spec: SubAgentSpec,
    app_config: Arc<AppConfig>,
    mut signal_rx: tokio::sync::watch::Receiver<Option<crate::sub_agents::types::MemoryAgentSignal>>,
) {
    let agent_id = &spec.agent_id;
    log::info!("Memory agent {}: started", agent_id);

    // Open memory store.
    let memory_store = match crate::memory::store::MemoryStore::open(
        crate::agentjax_home::agentjax_home_dir()
            .unwrap_or_else(|_| std::path::PathBuf::from("."))
            .join("memory"),
    ) {
        Ok(store) => store,
        Err(e) => {
            log::error!("Memory agent {}: failed to open memory store: {e}", agent_id);
            return;
        }
    };

    loop {
        // Wait for a signal.
        let signal = {
            let changed = signal_rx.changed().await;
            if changed.is_err() {
                log::info!("Memory agent {}: signal channel closed, exiting", agent_id);
                break;
            }
            signal_rx.borrow().clone()
        };

        match signal {
            Some(crate::sub_agents::types::MemoryAgentSignal::Terminate) | None => {
                log::info!("Memory agent {}: received Terminate, exiting", agent_id);
                break;
            }
            Some(crate::sub_agents::types::MemoryAgentSignal::TurnCompleted) => {
                log::info!("Memory agent {}: evaluating turn for memories", agent_id);

                // Build the memory index as context.
                let index_content = match crate::memory::index::MemoryIndex::rebuild(&memory_store)
                {
                    Ok(idx) => idx,
                    Err(e) => {
                        log::warn!("Memory agent {}: failed to rebuild index: {e}", agent_id);
                        continue;
                    }
                };

                // Build the prompt instructing the LLM to classify and act.
                let prompt = build_memory_agent_prompt(&index_content);

                // Resolve the model (use utility_small_model or default).
                let model_id = if app_config.utility_small_model.is_empty() {
                    app_config.default_model.clone()
                } else {
                    app_config.utility_small_model.clone()
                };

                // Call the provider.
                let request = crate::provider_api::types::ResponseStreamRequest {
                    input_items: vec![serde_json::json!({
                        "role": "user",
                        "content": [{"type": "input_text", "text": prompt}]
                    })],
                    model: Some(model_id),
                    reasoning_effort: None,
                    instructions_override: None,
                    text: None,
                    include: None,
                    service_tier: None,
                    prompt_cache_key: None,
                    client_metadata: None,
                    generate: None,
                    tools: None,
                    tool_choice: None,
                };

                let (_cancel_tx, mut cancel_rx) = tokio::sync::watch::channel(false);
                match crate::provider_api::stream_response(
                    &app_config,
                    &request,
                    &mut cancel_rx,
                    |_event| Ok(()),
                )
                .await
                {
                    Ok(response) => {
                        let text = response.output_text.trim().to_string();
                        if text.is_empty() {
                            continue;
                        }
                        // Parse the LLM output for memory operations.
                        execute_memory_operations(
                            &memory_store, &text, agent_id, &spec.parent_conversation_id,
                        );
                    }
                    Err(e) => {
                        log::warn!("Memory agent {}: LLM call failed: {e}", agent_id);
                    }
                }
            }
        }
    }
}

/// Build the classification prompt for the memory agent.
fn build_memory_agent_prompt(index_content: &str) -> String {
    let has_memories = index_content.contains("\n## ");
    let memory_context = if has_memories {
        format!(
            "## Existing Memory Index\n\n{}\n\n---\n",
            index_content
        )
    } else {
        "No existing memories found.\n\n".to_string()
    };

    format!(
        "You are a background memory observer agent. Your role is to review the \
         conversation and manage persistent memories across sessions.\n\n\
         {memory_context}\
         ## Instructions\n\n\
         Review the conversation context above and decide if there is any new, \
         corrected, or updated information worth remembering across sessions.\n\n\
         Classify each insight into ONE of:\n\
         - CREATE: Entirely new topic → write a new memory with a kebab-case name, \
           a one-line description, a type (user/feedback/project/reference), and the body.\n\
         - APPEND: Existing memory about the same topic, new developments → append new \
           info to the existing body. Include the full updated content.\n\
         - UPDATE: Existing memory needs correction (user corrected previous info) → \
           replace the content.\n\
         - IGNORE: No new memory-worthy content in this conversation.\n\n\
         ## Rules\n\
         1. ALWAYS check existing memories before CREATE to avoid duplicates.\n\
         2. When APPENDing, include the original content + new info together.\n\
         3. User corrections are high-priority UPDATE signals.\n\
         4. Casual conversation, small talk → IGNORE.\n\
         5. Technical preferences, project conventions, architectural decisions → CREATE.\n\n\
         ## Output Format\n\n\
         If IGNORE, output exactly: {{\"action\": \"ignore\"}}\n\n\
         Otherwise, output a JSON object with the memory operation:\n\
         {{\"action\": \"create\", \"name\": \"kebab-case-name\", \"description\": \"...\", \
         \"type\": \"project\", \"tags\": [\"tag1\"], \"body\": \"markdown body\"}}\n\n\
         For APPEND: {{\"action\": \"append\", \"name\": \"existing-name\", \"body\": \"full updated body\"}}\n\
         For UPDATE: {{\"action\": \"update\", \"name\": \"existing-name\", \"body\": \"corrected body\"}}\n\n\
         Output ONLY valid JSON. No markdown fences, no commentary."
    )
}

/// Parse the LLM output and execute memory operations via MemoryStore.
fn execute_memory_operations(
    store: &crate::memory::store::MemoryStore,
    llm_output: &str,
    agent_id: &str,
    parent_conv_id: &str,
) {
    // Try to extract JSON from the output.
    let json_str = llm_output
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```")
        .trim();

    let parsed: serde_json::Value = match serde_json::from_str(json_str) {
        Ok(v) => v,
        Err(e) => {
            log::warn!("Memory agent {}: failed to parse LLM output as JSON: {e}", agent_id);
            return;
        }
    };

    let action = parsed.get("action").and_then(|v| v.as_str()).unwrap_or("ignore");

    match action {
        "ignore" => {
            log::info!("Memory agent {}: IGNORE — no memories written", agent_id);
        }
        "create" | "append" | "update" => {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = parsed.get("description").and_then(|v| v.as_str()).unwrap_or("");
            let body = parsed.get("body").and_then(|v| v.as_str()).unwrap_or("");
            let memory_type_str = parsed.get("type").and_then(|v| v.as_str()).unwrap_or("project");
            let tags: Vec<String> = parsed
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| a.iter().filter_map(|t| t.as_str().map(String::from)).collect())
                .unwrap_or_default();

            if name.is_empty() || body.is_empty() {
                log::warn!("Memory agent {}: {action} missing name or body", agent_id);
                return;
            }

            let memory_type = crate::memory::types::MemoryType::from_str(memory_type_str)
                .unwrap_or(crate::memory::types::MemoryType::Project);

            // For append/update, read existing memory first.
            let final_body = if action == "append" {
                match store.read_memory(name) {
                    Ok(existing) => format!("{}\n\n{}", existing.body, body),
                    Err(_) => body.to_string(),
                }
            } else {
                body.to_string()
            };

            let links = extract_wikilinks_from_body(&final_body);

            let memory = crate::memory::types::ParsedMemory {
                frontmatter: crate::memory::types::MemoryFrontmatter {
                    name: name.to_string(),
                    description: description.to_string(),
                    memory_type,
                    tags,
                    links,
                },
                body: final_body,
            };

            match store.write_memory(&memory) {
                Ok(()) => {
                    log::info!("Memory agent {}: {action}d memory '{}'", agent_id, name);
                    // Deposit into Street for proactive context injection.
                    crate::street::StreetManager::deposit(
                        crate::street::StreetItem::new(
                            parent_conv_id,
                            crate::street::StreetSource::MemoryAgent,
                            crate::street::Priority::Low,
                            &format!("Memory updated: '{}' ({})", name, action),
                            serde_json::json!({"action": action, "name": name, "type": memory_type_str}),
                        ),
                    );
                }
                Err(e) => {
                    log::warn!("Memory agent {}: failed to {action} memory '{}': {e}", agent_id, name);
                }
            }
        }
        other => {
            log::warn!("Memory agent {}: unknown action '{}'", agent_id, other);
        }
    }
}

/// Extract `[[wikilinks]]` from the body text.
fn extract_wikilinks_from_body(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut remaining = body;
    while let Some(start) = remaining.find("[[") {
        let after_open = &remaining[start + 2..];
        if let Some(end) = after_open.find("]]") {
            let link = after_open[..end].trim();
            if !link.is_empty() {
                links.push(link.to_string());
            }
            remaining = &after_open[end + 2..];
        } else {
            break;
        }
    }
    links
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
