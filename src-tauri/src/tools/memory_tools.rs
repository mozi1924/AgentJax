//! Memory tools — `memory_write`, `memory_search`, `memory_recall`.
//!
//! These tools give the main agent the ability to write persistent memories,
//! search across all memories, and recall specific memories by name.

use crate::agentjax_home;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::memory::search::search_memories;
use crate::memory::store::MemoryStore;
use crate::memory::types::{MemoryFrontmatter, MemoryType, ParsedMemory};
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};

// ── Helpers ───────────────────────────────────────────────────────────────────

/// Get the memory store instance, creating the directory if needed.
///
/// When running within an agent context, uses the configured `storage_dir`
/// from `AgentConfig.memory`. Falls back to `"memory"` when no config is
/// available (e.g., tests).
fn open_memory_store(agent_config: Option<&crate::config::AgentConfig>) -> AgentJaxResult<MemoryStore> {
    let base_dir = agentjax_home::agentjax_home_dir()
        .map_err(|e| AgentJaxError::memory(format!("Failed to get agentjax home: {e}")))?
        .join(
            agent_config
                .map(|c| c.memory.storage_dir.as_str())
                .unwrap_or("memory"),
        );
    MemoryStore::open(base_dir)
}

// ── MemoryWriteTool ───────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryWriteArgs {
    name: String,
    description: String,
    #[serde(default = "default_memory_type")]
    memory_type: String,
    #[serde(default)]
    tags: Vec<String>,
    body: String,
}

fn default_memory_type() -> String {
    "project".to_string()
}

pub struct MemoryWriteTool;

#[async_trait::async_trait]
impl Tool for MemoryWriteTool {
    fn name(&self) -> &'static str {
        "memory_write"
    }

    fn description(&self) -> &'static str {
        "Write a persistent memory entry. Memories survive across conversations \
         and can be searched and recalled later. Use this to store project \
         knowledge, user preferences, decisions, or reference information. \
         The 'name' should be a short kebab-case slug. The body can contain \
         markdown with [[wikilinks]] to other memories."
    }

    fn display_name(&self) -> &'static str {
        "Write Memory"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Brain")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short kebab-case slug for this memory (e.g., 'project-architecture')."
                },
                "description": {
                    "type": "string",
                    "description": "One-line summary used to decide relevance during recall."
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
                    "description": "Optional tags for categorization."
                },
                "body": {
                    "type": "string",
                    "description": "The memory content. Can include [[wikilinks]] to other memories."
                }
            },
            "required": ["name", "description", "body"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: MemoryWriteArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::memory(format!("Invalid arguments: {e}")))?;

        let store = open_memory_store(context.agent_config.as_deref())?;

        let memory_type = MemoryType::from_str(&args.memory_type).unwrap_or(MemoryType::Project);

        // Extract [[wikilinks]] from body.
        let links = extract_wikilinks(&args.body);

        let memory = ParsedMemory {
            frontmatter: MemoryFrontmatter {
                name: args.name.clone(),
                description: args.description,
                memory_type,
                tags: args.tags,
                links,
            },
            body: args.body,
        };

        let existed = store.memory_exists(&args.name);
        store.write_memory(&memory)?;

        Ok(json!({
            "ok": true,
            "name": args.name,
            "action": if existed { "updated" } else { "created" },
            "hint": "Memory saved. Use memory_recall(name) to retrieve it, or memory_search(query) to find memories."
        }))
    }
}

// ── MemorySearchTool ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemorySearchArgs {
    query: String,
    #[serde(default = "default_max_results")]
    max_results: usize,
}

fn default_max_results() -> usize {
    10
}

pub struct MemorySearchTool;

#[async_trait::async_trait]
impl Tool for MemorySearchTool {
    fn name(&self) -> &'static str {
        "memory_search"
    }

    fn description(&self) -> &'static str {
        "Search across all stored memories. Returns ranked results with snippets. \
         Use this to find relevant memories before recalling them with memory_recall."
    }

    fn display_name(&self) -> &'static str {
        "Search Memory"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Search")
    }

    fn parameters_schema(&self) -> Value {
        json!({
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
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: MemorySearchArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::memory(format!("Invalid arguments: {e}")))?;

        let store = open_memory_store(context.agent_config.as_deref())?;
        let results = search_memories(&store, &args.query, args.max_results)?;

        Ok(json!({
            "ok": true,
            "query": args.query,
            "totalResults": results.len(),
            "results": results,
            "hint": "Use memory_recall(name) to retrieve the full content of any result."
        }))
    }
}

// ── MemoryRecallTool ──────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MemoryRecallArgs {
    name: String,
}

pub struct MemoryRecallTool;

#[async_trait::async_trait]
impl Tool for MemoryRecallTool {
    fn name(&self) -> &'static str {
        "memory_recall"
    }

    fn description(&self) -> &'static str {
        "Recall the full content of a specific memory by name. \
         Use memory_search first if you don't know the exact name."
    }

    fn display_name(&self) -> &'static str {
        "Recall Memory"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("BookOpen")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The name (kebab-case slug) of the memory to recall."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: MemoryRecallArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| AgentJaxError::memory(format!("Invalid arguments: {e}")))?;

        let store = open_memory_store(context.agent_config.as_deref())?;
        let memory = store.read_memory(&args.name)?;

        Ok(json!({
            "ok": true,
            "name": memory.frontmatter.name,
            "description": memory.frontmatter.description,
            "type": memory.frontmatter.memory_type.as_str(),
            "tags": memory.frontmatter.tags,
            "links": memory.frontmatter.links,
            "body": memory.body,
        }))
    }
}

// ── Wikilink extraction ───────────────────────────────────────────────────────

fn extract_wikilinks(body: &str) -> Vec<String> {
    let mut links = Vec::new();
    let mut remaining = body;
    while let Some(start) = remaining.find("[[") {
        let after_open = &remaining[start + 2..];
        if let Some(end) = after_open.find("]]") {
            let link = &after_open[..end].trim();
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

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_wikilinks() {
        let body = "See [[project-architecture]] and [[sub-agent-design]] for details.";
        let links = extract_wikilinks(body);
        assert_eq!(links, vec!["project-architecture", "sub-agent-design"]);
    }

    #[test]
    fn test_extract_wikilinks_empty() {
        let body = "No links here.";
        let links = extract_wikilinks(body);
        assert!(links.is_empty());
    }

    #[test]
    fn test_extract_wikilinks_malformed() {
        let body = "Broken [[link without close";
        let links = extract_wikilinks(body);
        assert!(links.is_empty());
    }

    #[test]
    fn test_memory_write_args_deserialization() {
        let args = json!({
            "name": "test-memory",
            "description": "A test",
            "memoryType": "project",
            "tags": ["test"],
            "body": "Content here."
        });
        let parsed: MemoryWriteArgs = serde_json::from_value(args).unwrap();
        assert_eq!(parsed.name, "test-memory");
        assert_eq!(parsed.memory_type, "project");
        assert_eq!(parsed.body, "Content here.");
    }
}
