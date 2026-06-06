//! Knowledge base tools — `kb_list`, `kb_search`, `kb_get`, `kb_index`.
//!
//! These tools give the agent the ability to list available knowledge bases,
//! perform hybrid (keyword + vector) searches, retrieve documents, and index
//! new content into a knowledge base.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::rag::KnowledgeBaseManager;
use crate::rag::types::Document;
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use tokio::sync::Mutex;

// ── Helpers ──────────────────────────────────────────────────────────────────

/// Create a knowledge base manager from the execution context.
fn kb_manager_from_ctx(ctx: &ToolExecutionContext) -> AgentJaxResult<KnowledgeBaseManager> {
    let app_config = ctx
        .app_config
        .as_ref()
        .ok_or_else(|| AgentJaxError::tool("No app config available"))?;
    let agent_config = ctx
        .agent_config
        .as_ref()
        .ok_or_else(|| AgentJaxError::tool("No agent config available"))?;
    KnowledgeBaseManager::from_config(app_config, agent_config)
}

// Lazy-initialized KB manager shared across KB tool calls.
static KB_MANAGER: std::sync::LazyLock<Mutex<Option<KnowledgeBaseManager>>> =
    std::sync::LazyLock::new(|| Mutex::new(None));

async fn get_or_init_kb_manager(
    ctx: &ToolExecutionContext,
) -> AgentJaxResult<tokio::sync::MutexGuard<'_, Option<KnowledgeBaseManager>>> {
    let mut guard = KB_MANAGER.lock().await;
    if guard.is_none() {
        *guard = Some(kb_manager_from_ctx(ctx)?);
    }
    Ok(guard)
}

// ── KbListTool ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KbListArgs {
    /// Optional filter: only show KBs whose name or ID contains this.
    #[serde(default)]
    query: Option<String>,
}

pub struct KbListTool;

#[async_trait::async_trait]
impl Tool for KbListTool {
    fn name(&self) -> &'static str {
        "kb_list"
    }

    fn description(&self) -> &'static str {
        "List all available knowledge bases. Returns metadata for each KB \
         including name, description, document count, and total size. \
         Knowledge bases are globally shared across all agent profiles."
    }

    fn display_name(&self) -> &'static str {
        "List Knowledge Bases"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Library")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Optional: filter KBs whose name or description contains this text."
                }
            },
            "required": []
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: KbListArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for kb_list: {e}"))?;

        let guard = get_or_init_kb_manager(context).await?;
        let manager = guard
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("KB manager not initialized"))?;
        let agent_config = context
            .agent_config
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("No agent config"))?;

        let mut kbs = manager.list_kbs(agent_config).await?;

        // Filter by query if provided
        if let Some(ref q) = args.query {
            let q = q.to_lowercase();
            kbs.retain(|kb| {
                kb.id.to_lowercase().contains(&q) || kb.name.to_lowercase().contains(&q)
            });
        }

        Ok(serde_json::to_value(&kbs).unwrap_or_default())
    }
}

// ── KbSearchTool ────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KbSearchArgs {
    /// The knowledge base ID to search.
    kb_id: String,
    /// The search query text.
    query: String,
    /// Number of results to return (default: 10, max: 50).
    #[serde(default = "default_top_k")]
    top_k: usize,
}

fn default_top_k() -> usize {
    10
}

pub struct KbSearchTool;

#[async_trait::async_trait]
impl Tool for KbSearchTool {
    fn name(&self) -> &'static str {
        "kb_search"
    }

    fn description(&self) -> &'static str {
        "Search a knowledge base using hybrid (keyword + semantic) retrieval. \
         Returns the most relevant document chunks ranked by combined score. \
         Use kb_list first to discover available knowledge base IDs."
    }

    fn display_name(&self) -> &'static str {
        "Search Knowledge Base"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Search")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kbId": {
                    "type": "string",
                    "description": "The knowledge base ID to search (use kb_list to discover)."
                },
                "query": {
                    "type": "string",
                    "description": "The search query — natural language or keywords."
                },
                "topK": {
                    "type": "integer",
                    "description": "Number of results (default 10, max 50).",
                    "default": 10
                }
            },
            "required": ["kbId", "query"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: KbSearchArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for kb_search: {e}"))?;

        let top_k = args.top_k.clamp(1, 50);
        let guard = get_or_init_kb_manager(context).await?;
        let manager = guard
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("KB manager not initialized"))?;
        let app_config = context
            .app_config
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("No app config"))?;

        let results = manager.search(&args.kb_id, &args.query, top_k, app_config).await?;

        Ok(serde_json::to_value(&results).unwrap_or_default())
    }
}

// ── KbGetTool ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KbGetArgs {
    /// The knowledge base ID.
    kb_id: String,
    /// The document ID to retrieve.
    document_id: String,
}

pub struct KbGetTool;

#[async_trait::async_trait]
impl Tool for KbGetTool {
    fn name(&self) -> &'static str {
        "kb_get"
    }

    fn description(&self) -> &'static str {
        "Retrieve the full content of a document from a knowledge base by its ID. \
         Returns all chunks assembled in order. Use kb_search first to find \
         relevant document IDs."
    }

    fn display_name(&self) -> &'static str {
        "Get KB Document"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("FileText")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kbId": {
                    "type": "string",
                    "description": "The knowledge base ID."
                },
                "documentId": {
                    "type": "string",
                    "description": "The document ID to retrieve (found in search results)."
                }
            },
            "required": ["kbId", "documentId"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: KbGetArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for kb_get: {e}"))?;

        let guard = get_or_init_kb_manager(context).await?;
        let manager = guard
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("KB manager not initialized"))?;

        let chunks = manager.get_document(&args.kb_id, &args.document_id).await?;

        if chunks.is_empty() {
            return Ok(json!({
                "found": false,
                "documentId": args.document_id,
                "content": ""
            }));
        }

        // Reassemble document from chunks (they may overlap)
        let full_content = chunks.join("\n");

        Ok(json!({
            "found": true,
            "documentId": args.document_id,
            "chunks": chunks.len(),
            "content": full_content
        }))
    }
}

// ── KbIndexTool ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct KbIndexArgs {
    /// The knowledge base ID to index into.
    kb_id: String,
    /// The document ID (unique identifier within this KB).
    document_id: String,
    /// The document content to index (markdown recommended).
    content: String,
    /// Optional metadata (title, source, tags, etc.).
    #[serde(default)]
    metadata: BTreeMap<String, String>,
}

pub struct KbIndexTool;

#[async_trait::async_trait]
impl Tool for KbIndexTool {
    fn name(&self) -> &'static str {
        "kb_index"
    }

    fn description(&self) -> &'static str {
        "Index a document into a knowledge base. This is a long-running \
         operation that chunks, embeds, and stores the document. \
         Documents are deduplicated by content hash — re-indexing \
         identical content is a no-op. Returns progress information."
    }

    fn display_name(&self) -> &'static str {
        "Index KB Document"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Upload")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "kbId": {
                    "type": "string",
                    "description": "The knowledge base ID to index into."
                },
                "documentId": {
                    "type": "string",
                    "description": "A unique identifier for this document within the KB (e.g., a filename or slug)."
                },
                "content": {
                    "type": "string",
                    "description": "The full document content to index. Markdown format is recommended for best chunking results."
                },
                "metadata": {
                    "type": "object",
                    "description": "Optional key-value metadata (e.g., {\"title\": \"My Doc\", \"source\": \"url\", \"tags\": \"ai,ml\"}).",
                    "additionalProperties": { "type": "string" }
                }
            },
            "required": ["kbId", "documentId", "content"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: KbIndexArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for kb_index: {e}"))?;

        let guard = get_or_init_kb_manager(context).await?;
        let manager = guard
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("KB manager not initialized"))?;
        let app_config = context
            .app_config
            .as_ref()
            .ok_or_else(|| AgentJaxError::tool("No app config"))?;

        let document = Document {
            id: args.document_id,
            content: args.content,
            metadata: args.metadata,
        };

        let progress = manager.index_document(&args.kb_id, document, app_config).await?;

        Ok(serde_json::to_value(&progress).unwrap_or_default())
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_kb_list_schema() {
        let tool = KbListTool;
        let schema = tool.parameters_schema();
        assert_eq!(schema["type"], "object");
    }

    #[test]
    fn test_kb_search_schema() {
        let tool = KbSearchTool;
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("kbId")));
        assert!(required.iter().any(|v| v.as_str() == Some("query")));
    }

    #[test]
    fn test_kb_get_schema() {
        let tool = KbGetTool;
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("kbId")));
        assert!(required.iter().any(|v| v.as_str() == Some("documentId")));
    }

    #[test]
    fn test_kb_index_schema() {
        let tool = KbIndexTool;
        let schema = tool.parameters_schema();
        let required = schema["required"].as_array().unwrap();
        assert!(required.iter().any(|v| v.as_str() == Some("kbId")));
        assert!(required.iter().any(|v| v.as_str() == Some("documentId")));
        assert!(required.iter().any(|v| v.as_str() == Some("content")));
    }
}
