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

fn open_memory_store() -> Result<MemoryStore, AgentJaxError> {
    let base_dir = agentjax_home::agentjax_home_dir()
        .map_err(AgentJaxError::memory)?
        .join("memory");
    MemoryStore::open(base_dir).map_err(AgentJaxError::memory)
}

/// List all memory index entries.
#[tauri::command]
pub fn list_memories() -> Result<Vec<MemoryIndexEntry>, AgentJaxError> {
    let store = open_memory_store()?;
    store.list_memories().map_err(AgentJaxError::memory)
}

/// Get the full content of a specific memory by name.
#[tauri::command]
pub fn get_memory(req: GetMemoryRequest) -> Result<serde_json::Value, AgentJaxError> {
    let store = open_memory_store()?;
    let memory = store.read_memory(&req.name).map_err(AgentJaxError::memory)?;
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
pub fn search_memories(req: SearchMemoriesRequest) -> Result<serde_json::Value, AgentJaxError> {
    let store = open_memory_store()?;
    let results = search_memories_impl(&store, &req.query, req.max_results)
        .map_err(AgentJaxError::memory)?;
    Ok(serde_json::json!({
        "ok": true,
        "query": req.query,
        "totalResults": results.len(),
        "results": results,
    }))
}

/// Delete a memory by name.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteMemoryRequest {
    pub name: String,
}

#[tauri::command]
pub fn delete_memory(req: DeleteMemoryRequest) -> Result<serde_json::Value, AgentJaxError> {
    let store = open_memory_store()?;
    let existed = store.delete_memory(&req.name).map_err(AgentJaxError::memory)?;
    Ok(serde_json::json!({
        "ok": true,
        "name": req.name,
        "existed": existed,
    }))
}

/// Open a memory file in the system default editor.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenMemoryFileRequest {
    pub name: String,
}

#[tauri::command]
pub fn open_memory_file(req: OpenMemoryFileRequest) -> Result<serde_json::Value, AgentJaxError> {
    let base_dir = crate::agentjax_home::agentjax_home_dir()
        .map_err(AgentJaxError::memory)?
        .join("memory");
    let file_path = base_dir.join(format!("{}.md", req.name));
    if !file_path.exists() {
        return Err(AgentJaxError::not_found(format!("Memory file not found: {}", file_path.display())));
    }
    #[cfg(target_os = "macos")]
    {
        std::process::Command::new("open")
            .arg(&file_path)
            .spawn()
            .map_err(|e| AgentJaxError::internal(format!("Failed to open file: {e}")))?;
    }
    #[cfg(target_os = "linux")]
    {
        std::process::Command::new("xdg-open")
            .arg(&file_path)
            .spawn()
            .map_err(|e| AgentJaxError::internal(format!("Failed to open file: {e}")))?;
    }
    #[cfg(target_os = "windows")]
    {
        std::process::Command::new("cmd")
            .args(["/C", "start", "", file_path.to_str().unwrap_or("")])
            .spawn()
            .map_err(|e| AgentJaxError::internal(format!("Failed to open file: {e}")))?;
    }
    Ok(serde_json::json!({
        "ok": true,
        "name": req.name,
    }))
}
