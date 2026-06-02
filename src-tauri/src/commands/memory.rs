//! Tauri IPC commands for memory management.

use crate::agentjax_home;
use crate::error::AgentJaxError;
use crate::memory::search::search_memories as search_memories_impl;
use crate::memory::store::MemoryStore;
use crate::memory::types::MemoryIndexEntry;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetMemoryRequest {
    pub name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchMemoriesRequest {
    pub query: String,
    #[serde(default = "default_max_results")]
    pub max_results: usize,
}

fn default_max_results() -> usize {
    10
}

fn open_memory_store() -> Result<MemoryStore, String> {
    let base_dir = agentjax_home::agentjax_home_dir()
        .map_err(|e| AgentJaxError::memory(e).to_string())?
        .join("memory");
    MemoryStore::open(base_dir).map_err(|e| e.to_string())
}

/// List all memory index entries.
#[tauri::command]
pub fn list_memories() -> Result<Vec<MemoryIndexEntry>, String> {
    let store = open_memory_store()?;
    store.list_memories().map_err(|e| e.to_string())
}

/// Get the full content of a specific memory by name.
#[tauri::command]
pub fn get_memory(req: GetMemoryRequest) -> Result<serde_json::Value, String> {
    let store = open_memory_store()?;
    let memory = store.read_memory(&req.name).map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "name": memory.frontmatter.name,
        "description": memory.frontmatter.description,
        "type": memory.frontmatter.memory_type.as_str(),
        "tags": memory.frontmatter.tags,
        "links": memory.frontmatter.links,
        "body": memory.body,
    }))
}

/// Search across all memories.
#[tauri::command]
pub fn search_memories(req: SearchMemoriesRequest) -> Result<serde_json::Value, String> {
    let store = open_memory_store()?;
    let results = search_memories_impl(&store, &req.query, req.max_results)
        .map_err(|e| e.to_string())?;
    Ok(serde_json::json!({
        "ok": true,
        "query": req.query,
        "totalResults": results.len(),
        "results": results,
    }))
}
