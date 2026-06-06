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
use crate::runtime::agent_context::{AgentContext, InMemoryContext};
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

// ── Memory Agent Runner ───────────────────────────────────────────────────────

/// Run the persistent background memory sub-agent.
///
/// Unlike `run_sub_agent`, this is event-driven: it waits for signals
/// (TurnCompleted, Terminate) on a watch channel, evaluates conversation
/// context, and writes memories directly via MemoryStore.
pub async fn run_memory_agent(
    spec: SubAgentSpec,
    app_config: Arc<AppConfig>,
    agent_config: Arc<AgentConfig>,
    mut signal_rx: tokio::sync::watch::Receiver<
        Option<crate::sub_agents::types::MemoryAgentSignal>,
    >,
) {
    let agent_id = &spec.agent_id;
    log::info!("Memory agent {}: started", agent_id);

    // Open memory store.
    let memory_store: std::sync::Arc<crate::memory::store::MemoryStore> =
        match crate::memory::store::MemoryStore::open(
            crate::agentjax_home::agentjax_home_dir()
                .unwrap_or_else(|_| std::path::PathBuf::from("."))
                .join("memory"),
        ) {
            Ok(store) => std::sync::Arc::new(store),
            Err(e) => {
                log::error!(
                    "Memory agent {}: failed to open memory store: {e}",
                    agent_id
                );
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

                // Load parent conversation context via MemoryAgentContext.
                let mem_ctx = crate::runtime::agent_context::MemoryAgentContext::new(
                    &spec.parent_conversation_id,
                );
                if let Err(e) = mem_ctx.rebuild(&spec.parent_conversation_id).await {
                    log::warn!("Memory agent {}: failed to load LCM context: {e}", agent_id);
                    continue;
                }
                let context_items = mem_ctx.context_items();

                // Build the prompt with only the memory index (context is injected separately).
                let prompt = build_memory_agent_prompt(&index_content);

                // Resolve the model (use utility_small_model or default).
                let model_id = if agent_config.utility_small_model.is_empty() {
                    agent_config.default_model.clone()
                } else {
                    agent_config.utility_small_model.clone()
                };

                // Define memory tools for the agent loop.
                let tool_definitions = vec![
                    memory_search_schema(),
                    memory_recall_schema(),
                    memory_write_schema(),
                ];

                // Create tool handlers backed by the memory store.
                let tool_handlers: Vec<Box<dyn ToolHandler>> = vec![
                    Box::new(MemorySearchHandler {
                        store: memory_store.clone(),
                    }),
                    Box::new(MemoryRecallHandler {
                        store: memory_store.clone(),
                    }),
                    Box::new(MemoryWriteHandler {
                        store: memory_store.clone(),
                        parent_conv_id: spec.parent_conversation_id.clone(),
                    }),
                ];

                match crate::runtime::tool_loop::run_tool_loop(
                    &prompt,
                    context_items,
                    tool_definitions,
                    tool_handlers,
                    &model_id,
                    3,
                    &app_config,
                    &agent_config,
                )
                .await
                {
                    Ok(text) => {
                        if text.is_empty() || text.contains(r#""action": "ignore""#) {
                            log::info!("Memory agent {}: IGNORE — no memories written", agent_id);
                            continue;
                        }
                        execute_memory_operations(
                            &memory_store,
                            &text,
                            agent_id,
                            &spec.parent_conversation_id,
                        );
                    }
                    Err(e) => {
                        log::warn!("Memory agent {}: tool loop failed: {e}", agent_id);
                    }
                }
            }
        }
    }
}

/// Build the classification prompt for the memory agent.
///
/// The prompt includes the memory index and instructions. Conversation
/// context is injected separately via `MemoryAgentContext` as provider items.
fn build_memory_agent_prompt(index_content: &str) -> String {
    let has_memories = index_content.contains("\n## ");
    let memory_context = if has_memories {
        format!("## Existing Memory Index\n\n{}\n\n---\n", index_content)
    } else {
        "No existing memories found.\n\n---\n".to_string()
    };

    format!(
        "You are a background memory observer agent. Your role is to review the \
         conversation and manage persistent memories across sessions.\n\n\
         {memory_context}\
         ## Instructions\n\n\
         Review the recent conversation context (provided below) and decide if there \
         is any new, corrected, or updated information worth remembering.\n\n\
         You have access to memory tools:\n\
         - **memory_search(query)**: Search existing memories before creating new ones.\n\
         - **memory_recall(name)**: Read the full content of a specific memory.\n\
         - **memory_write(name, description, type, tags, body)**: Create or update a memory.\n\n\
         ## Workflow\n\n\
         1. Use memory_search to check for existing memories related to the topic.\n\
         2. If you find an existing memory that needs updating, use memory_recall \
           to read it, then memory_write with the updated content.\n\
         3. For entirely new information, use memory_write directly.\n\
         4. If nothing noteworthy happened, output: {{\"action\": \"ignore\"}}\n\n\
         ## Memory Types\n\
         - **user**: User preferences, personal info, habits\n\
         - **feedback**: User corrections, complaints, suggestions\n\
         - **project**: Code architecture, conventions, decisions\n\
         - **reference**: External references (URLs, docs)\n\n\
         ## Output Format\n\n\
         When done, output a JSON object with the final result:\n\
         - For ignore: {{\"action\": \"ignore\"}}\n\
         - For create: {{\"action\": \"create\", \"name\": \"kebab-case-name\", ...}}\n\
         - For append: {{\"action\": \"append\", \"name\": \"existing-name\", \"body\": \"...\"}}\n\
         - For update: {{\"action\": \"update\", \"name\": \"existing-name\", \"body\": \"...\"}}\n\n\
         Output ONLY valid JSON at the end. No markdown fences, no commentary."
    )
}

// ── Memory Tool Schemas ──────────────────────────────────────────────────────

fn memory_search_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": "memory_search",
        "description": "Search across all stored memories. Returns ranked results with snippets.",
        "parameters": {
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query. Matches against name, description, tags, and body."
                },
                "maxResults": {
                    "type": "integer",
                    "description": "Maximum number of results (default 10).",
                    "default": 10
                }
            },
            "required": ["query"]
        }
    })
}

fn memory_recall_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": "memory_recall",
        "description": "Recall the full content of a specific memory by name.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name (kebab-case slug) of the memory to recall."
                }
            },
            "required": ["name"]
        }
    })
}

fn memory_write_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "function",
        "name": "memory_write",
        "description": "Write or update a persistent memory entry.",
        "parameters": {
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short kebab-case slug (e.g., 'project-architecture')."
                },
                "description": {
                    "type": "string",
                    "description": "One-line summary."
                },
                "memoryType": {
                    "type": "string",
                    "enum": ["user", "feedback", "project", "reference"],
                    "description": "The type of memory.",
                    "default": "project"
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags."
                },
                "body": {
                    "type": "string",
                    "description": "The memory content. Can include [[wikilinks]]."
                }
            },
            "required": ["name", "description", "body"]
        }
    })
}

// ── Memory Tool Handlers ──────────────────────────────────────────────────────

use crate::runtime::tool_loop::ToolHandler;

struct MemorySearchHandler {
    store: std::sync::Arc<crate::memory::store::MemoryStore>,
}

#[async_trait::async_trait]
impl ToolHandler for MemorySearchHandler {
    fn name(&self) -> &str {
        "memory_search"
    }

    async fn execute(&self, arguments: &str) -> String {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return format!("{{\"error\": \"Invalid arguments: {e}\"}}"),
        };
        let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
        let max_results = args
            .get("maxResults")
            .and_then(|v| v.as_u64())
            .unwrap_or(10) as usize;

        match crate::memory::search::search_memories(&self.store, query, max_results) {
            Ok(results) => serde_json::to_string(&results).unwrap_or_else(|_| "[]".to_string()),
            Err(e) => format!("{{\"error\": \"{e}\"}}"),
        }
    }
}

struct MemoryRecallHandler {
    store: std::sync::Arc<crate::memory::store::MemoryStore>,
}

#[async_trait::async_trait]
impl ToolHandler for MemoryRecallHandler {
    fn name(&self) -> &str {
        "memory_recall"
    }

    async fn execute(&self, arguments: &str) -> String {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return format!("{{\"error\": \"Invalid arguments: {e}\"}}"),
        };
        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");

        match self.store.read_memory(name) {
            Ok(memory) => serde_json::json!({
                "name": memory.frontmatter.name,
                "description": memory.frontmatter.description,
                "type": memory.frontmatter.memory_type.as_str(),
                "tags": memory.frontmatter.tags,
                "links": memory.frontmatter.links,
                "body": memory.body,
            })
            .to_string(),
            Err(e) => format!("{{\"error\": \"Memory '{name}' not found: {e}\"}}"),
        }
    }
}

struct MemoryWriteHandler {
    store: std::sync::Arc<crate::memory::store::MemoryStore>,
    parent_conv_id: String,
}

#[async_trait::async_trait]
impl ToolHandler for MemoryWriteHandler {
    fn name(&self) -> &str {
        "memory_write"
    }

    async fn execute(&self, arguments: &str) -> String {
        let args: serde_json::Value = match serde_json::from_str(arguments) {
            Ok(v) => v,
            Err(e) => return format!("{{\"error\": \"Invalid arguments: {e}\"}}"),
        };

        let name = args.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let body = args.get("body").and_then(|v| v.as_str()).unwrap_or("");
        let memory_type_str = args
            .get("memoryType")
            .and_then(|v| v.as_str())
            .unwrap_or("project");
        let tags: Vec<String> = args
            .get("tags")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|t| t.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let memory_type = crate::memory::types::MemoryType::from_str(memory_type_str)
            .unwrap_or(crate::memory::types::MemoryType::Project);

        if name.is_empty() || body.is_empty() {
            return "{{\"error\": \"Missing required fields: name, body\"}}".to_string();
        }

        let links = crate::sub_agents::runner::extract_wikilinks_from_body(body);
        let memory = crate::memory::types::ParsedMemory {
            frontmatter: crate::memory::types::MemoryFrontmatter {
                name: name.to_string(),
                description: description.to_string(),
                memory_type,
                tags,
                links,
            },
            body: body.to_string(),
        };

        match self.store.write_memory(&memory) {
            Ok(()) => {
                crate::street::StreetManager::deposit(crate::street::StreetItem::new(
                    &self.parent_conv_id,
                    crate::street::StreetSource::MemoryAgent,
                    crate::street::Priority::Low,
                    &format!("Memory updated: '{name}'"),
                    serde_json::json!({"action": "write", "name": name, "type": memory_type_str}),
                ));
                format!("{{\"ok\": true, \"name\": \"{name}\", \"action\": \"written\"}}")
            }
            Err(e) => format!("{{\"error\": \"Failed to write memory: {e}\"}}"),
        }
    }
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
            log::warn!(
                "Memory agent {}: failed to parse LLM output as JSON: {e}",
                agent_id
            );
            return;
        }
    };

    let action = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("ignore");

    match action {
        "ignore" => {
            log::info!("Memory agent {}: IGNORE — no memories written", agent_id);
        }
        "create" | "append" | "update" => {
            let name = parsed.get("name").and_then(|v| v.as_str()).unwrap_or("");
            let description = parsed
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let body = parsed.get("body").and_then(|v| v.as_str()).unwrap_or("");
            let memory_type_str = parsed
                .get("type")
                .and_then(|v| v.as_str())
                .unwrap_or("project");
            let tags: Vec<String> = parsed
                .get("tags")
                .and_then(|v| v.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|t| t.as_str().map(String::from))
                        .collect()
                })
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
                    crate::street::StreetManager::deposit(crate::street::StreetItem::new(
                        parent_conv_id,
                        crate::street::StreetSource::MemoryAgent,
                        crate::street::Priority::Low,
                        &format!("Memory updated: '{}' ({})", name, action),
                        serde_json::json!({"action": action, "name": name, "type": memory_type_str}),
                    ));
                }
                Err(e) => {
                    log::warn!(
                        "Memory agent {}: failed to {action} memory '{}': {e}",
                        agent_id,
                        name
                    );
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
