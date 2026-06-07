//! Inline memory evaluation — runs synchronously after each conversation turn.
//!
//! Unlike the old `run_memory_agent` sub-agent approach, this module evaluates
//! memory needs inline after each main turn completes. It:
//!
//! 1. Rebuilds the memory index for context
//! 2. Loads recent parent conversation context via `MemoryAgentContext`
//! 3. Runs a lightweight tool-calling loop with memory tools (search/recall/write)
//! 4. Executes any memory operations determined by the LLM
//!
//! This avoids:
//! - Registering a persistent sub-agent in `SubAgentManager` (no `mem_*` noise)
//! - Signal-based event dispatch (`TurnCompleted` / `Terminate`)
//! - Street notification deposits (silent operation)
//! - Filesystem artifacts from ephemeral agent IDs

use crate::config::{AgentConfig, AppConfig};
use crate::memory::store::MemoryStore;
use crate::memory::types::{MemoryFrontmatter, MemoryType, ParsedMemory};
use crate::runtime::agent_context::AgentContext;
use crate::runtime::agent_context::MemoryAgentContext;
use crate::runtime::tool_loop::{ToolHandler, run_tool_loop};
use serde_json::Value;
use std::sync::Arc;

/// Evaluate the recent conversation context and write memories if appropriate.
///
/// Called after each main turn completes. Opens the memory store, builds
/// a memory index, loads recent context, and runs a minimal tool loop.
/// This is intentionally **silent** — no street notifications, no frontend events.
pub async fn evaluate_and_write_memories(
    parent_conversation_id: &str,
    app_config: &AppConfig,
    agent_config: &AgentConfig,
) {
    // Open the memory store.
    let memory_store: Arc<MemoryStore> = match crate::agentjax_home::agentjax_home_dir() {
        Ok(home) => match MemoryStore::open(home.join("memory")) {
            Ok(store) => Arc::new(store),
            Err(e) => {
                log::warn!("Memory evaluation: failed to open memory store: {e}");
                return;
            }
        },
        Err(e) => {
            log::warn!("Memory evaluation: failed to resolve home dir: {e}");
            return;
        }
    };

    // Build the memory index as context.
    let index_content = match crate::memory::index::MemoryIndex::rebuild(&memory_store) {
        Ok(idx) => idx,
        Err(e) => {
            log::warn!("Memory evaluation: failed to rebuild index: {e}");
            return;
        }
    };

    // Load parent conversation context via MemoryAgentContext.
    let mem_ctx = MemoryAgentContext::new(parent_conversation_id);
    if let Err(e) = mem_ctx.rebuild(parent_conversation_id).await {
        log::warn!("Memory evaluation: failed to load LCM context: {e}");
        return;
    }
    let context_items = match mem_ctx.context_items().await {
        Ok(items) => items,
        Err(e) => {
            log::warn!("Memory evaluation: failed to get context items: {e}");
            return;
        }
    };
    if context_items.is_empty() {
        log::info!("Memory evaluation: no context items, skipping");
        return;
    }

    // Build the prompt with memory index.
    let prompt = build_memory_evaluation_prompt(&index_content);

    // Resolve the model (use utility_small_model or default).
    let model_id = if agent_config.utility_small_model.is_empty() {
        &agent_config.default_model
    } else {
        &agent_config.utility_small_model
    };
    if model_id.is_empty() {
        log::warn!("Memory evaluation: no model configured, skipping");
        return;
    }

    // Define memory tools for the tool loop.
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
        }),
    ];

    match run_tool_loop(
        &prompt,
        context_items,
        tool_definitions,
        tool_handlers,
        model_id,
        3,
        app_config,
        agent_config,
    )
    .await
    {
        Ok(text) => {
            if text.is_empty() || text.contains(r#""action": "ignore""#) {
                log::info!("Memory evaluation: IGNORE — no memories written");
                return;
            }
            execute_memory_operations(&memory_store, &text);
        }
        Err(e) => {
            log::warn!("Memory evaluation: tool loop failed: {e}");
        }
    }
}

/// Build the evaluation prompt for the memory LLM call.
fn build_memory_evaluation_prompt(index_content: &str) -> String {
    let has_memories = index_content.contains("\n## ");
    let memory_context = if has_memories {
        format!("## Existing Memory Index\n\n{}\n\n---\n", index_content)
    } else {
        "No existing memories found.\n\n---\n".to_string()
    };

    format!(
        "You are a background memory observer. Review the recent conversation \
         and decide if there is any new, corrected, or updated information \
         worth remembering.\n\n\
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

/// Parse the LLM output and execute memory operations via MemoryStore.
fn execute_memory_operations(
    store: &MemoryStore,
    llm_output: &str,
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
            log::warn!("Memory evaluation: failed to parse LLM output as JSON: {e}");
            return;
        }
    };

    let action = parsed
        .get("action")
        .and_then(|v| v.as_str())
        .unwrap_or("ignore");

    match action {
        "ignore" => {
            log::info!("Memory evaluation: IGNORE — no memories written");
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
                log::warn!("Memory evaluation: {action} missing name or body");
                return;
            }

            let memory_type = MemoryType::from_str(memory_type_str)
                .unwrap_or(MemoryType::Project);

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

            let memory = ParsedMemory {
                frontmatter: MemoryFrontmatter {
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
                    log::info!("Memory evaluation: {action}d memory '{name}'");
                    // Intentionally no street deposit — this is a silent evaluation.
                    // The memory will be picked up by the next context load.
                }
                Err(e) => {
                    log::warn!("Memory evaluation: failed to {action} memory '{name}': {e}");
                }
            }
        }
        other => {
            log::warn!("Memory evaluation: unknown action '{other}'");
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

// ── Memory Tool Schemas ──────────────────────────────────────────────────────

fn memory_search_schema() -> Value {
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

fn memory_recall_schema() -> Value {
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

fn memory_write_schema() -> Value {
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

// ── Memory Tool Handlers ─────────────────────────────────────────────────────

struct MemorySearchHandler {
    store: Arc<MemoryStore>,
}

#[async_trait::async_trait]
impl ToolHandler for MemorySearchHandler {
    fn name(&self) -> &str {
        "memory_search"
    }

    async fn execute(&self, arguments: &str) -> String {
        let args: Value = match serde_json::from_str(arguments) {
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
    store: Arc<MemoryStore>,
}

#[async_trait::async_trait]
impl ToolHandler for MemoryRecallHandler {
    fn name(&self) -> &str {
        "memory_recall"
    }

    async fn execute(&self, arguments: &str) -> String {
        let args: Value = match serde_json::from_str(arguments) {
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
    store: Arc<MemoryStore>,
}

#[async_trait::async_trait]
impl ToolHandler for MemoryWriteHandler {
    fn name(&self) -> &str {
        "memory_write"
    }

    async fn execute(&self, arguments: &str) -> String {
        let args: Value = match serde_json::from_str(arguments) {
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
        let memory_type = MemoryType::from_str(memory_type_str)
            .unwrap_or(MemoryType::Project);

        if name.is_empty() || body.is_empty() {
            return "{{\"error\": \"Missing required fields: name, body\"}}".to_string();
        }

        let links = extract_wikilinks_from_body(body);
        let memory = ParsedMemory {
            frontmatter: MemoryFrontmatter {
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
                // Intentionally no street deposit — silent evaluation.
                format!("{{\"ok\": true, \"name\": \"{name}\", \"action\": \"written\"}}")
            }
            Err(e) => format!("{{\"error\": \"Failed to write memory: {e}\"}}"),
        }
    }
}
