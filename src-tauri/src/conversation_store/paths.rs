use crate::config::constants::DEFAULT_AGENT_ID;
use crate::conversation_store_utils::sanitize_conversation_id;
use crate::error::{AgentJaxError, AgentJaxResult};
use std::fs;
use std::path::PathBuf;

const SESSIONS_DIR_NAME: &str = "sessions";
const METADATA_FILE_NAME: &str = "metadata.json";
const MESSAGES_FILE_NAME: &str = "messages.jsonl";
const LCM_DB_FILE_NAME: &str = "lcm.db";
const WORKSPACE_DIR_NAME: &str = "workspace";

/// Alias to agentjax home for convenience.
pub fn agentjax_home_dir() -> AgentJaxResult<PathBuf> {
    crate::agentjax_home::agentjax_home_dir()
}

/// `~/.agentjax/agents/{agent_id}/sessions/` — root for all conversations of an agent.
pub fn conversations_dir_path(agent_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(crate::agentjax_home::agent_dir(agent_id)?.join(SESSIONS_DIR_NAME))
}

pub fn ensure_conversations_dir(agent_id: &str) -> AgentJaxResult<PathBuf> {
    let dir = conversations_dir_path(agent_id)?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| AgentJaxError::internal(format!("Failed to create conversations dir {}: {e}", dir.display())).with_error_source(&e))?;
    }
    Ok(dir)
}

pub fn conversation_dir_path(agent_id: &str, conversation_id: &str) -> AgentJaxResult<PathBuf> {
    let dir = ensure_conversations_dir(agent_id)?;
    let safe = sanitize_conversation_id(conversation_id);
    Ok(dir.join(safe))
}

pub fn conversation_metadata_path(agent_id: &str, conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(agent_id, conversation_id)?.join(METADATA_FILE_NAME))
}

pub fn conversation_messages_path(agent_id: &str, conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(agent_id, conversation_id)?.join(MESSAGES_FILE_NAME))
}

pub fn conversation_workspace_path(agent_id: &str, conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(agent_id, conversation_id)?.join(WORKSPACE_DIR_NAME))
}

pub fn ensure_session_layout(agent_id: &str, conversation_id: &str) -> AgentJaxResult<()> {
    let workspace_dir = conversation_workspace_path(agent_id, conversation_id)?;
    if !workspace_dir.exists() {
        fs::create_dir_all(&workspace_dir).map_err(|e| {
            AgentJaxError::internal(format!(
                "Failed to create session workspace directory {}: {e}",
                workspace_dir.display()
            ))
            .with_error_source(&e)
        })?;
    }
    Ok(())
}

pub fn conversation_lcm_db_path(agent_id: &str, conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(agent_id, conversation_id)?.join(LCM_DB_FILE_NAME))
}

pub fn list_conversation_ids(agent_id: &str) -> AgentJaxResult<Vec<String>> {
    let dir = ensure_conversations_dir(agent_id)?;
    let mut out = Vec::new();

    let entries = fs::read_dir(&dir)
        .map_err(|e| AgentJaxError::internal(format!("Failed to read conversations dir {}: {e}", dir.display())).with_error_source(&e))?;

    for entry in entries {
        let entry = entry.map_err(|e| AgentJaxError::internal(format!("Failed to inspect conversation file entry: {e}")).with_error_source(&e))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }
        // Accept conversation if it has EITHER legacy files OR an LCM database.
        let metadata = path.join(METADATA_FILE_NAME);
        let messages = path.join(MESSAGES_FILE_NAME);
        let lcm_db = path.join(LCM_DB_FILE_NAME);
        let has_legacy = metadata.exists() && messages.exists();
        let has_lcm = lcm_db.exists();
        if !has_legacy && !has_lcm {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|s| s.to_str())
            && !name.trim().is_empty() {
                out.push(name.to_string());
            }
    }

    Ok(out)
}

// ── Default agent convenience shims ───────────────────────────────────────

/// Convenience: calls `conversations_dir_path(DEFAULT_AGENT_ID)` for tests/backfill.
pub fn default_default_conversations_dir_path() -> AgentJaxResult<PathBuf> {
    conversations_dir_path(DEFAULT_AGENT_ID)
}

/// Convenience: calls `conversation_dir_path(DEFAULT_AGENT_ID, conversation_id)`.
pub fn default_conversation_dir_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    conversation_dir_path(DEFAULT_AGENT_ID, conversation_id)
}

/// Convenience: calls `conversation_workspace_path(DEFAULT_AGENT_ID, conversation_id)`.
pub fn default_conversation_workspace_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    conversation_workspace_path(DEFAULT_AGENT_ID, conversation_id)
}

/// Convenience: calls `list_conversation_ids(DEFAULT_AGENT_ID)`.
pub fn default_list_conversation_ids() -> AgentJaxResult<Vec<String>> {
    list_conversation_ids(DEFAULT_AGENT_ID)
}
