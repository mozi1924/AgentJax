use crate::conversation_store_utils::sanitize_conversation_id;
use crate::error::{AgentJaxError, AgentJaxResult};
use std::fs;
use std::path::PathBuf;

const SESSIONS_DIR_NAME: &str = "sessions";
const METADATA_FILE_NAME: &str = "metadata.json";
const MESSAGES_FILE_NAME: &str = "messages.jsonl";
const LCM_DB_FILE_NAME: &str = "lcm.db";
const WORKSPACE_DIR_NAME: &str = "workspace";

pub fn agentjax_home_dir() -> AgentJaxResult<PathBuf> {
    crate::agentjax_home::agentjax_home_dir().map_err(Into::into)
}

pub fn conversations_dir_path() -> AgentJaxResult<PathBuf> {
    Ok(agentjax_home_dir()?.join(SESSIONS_DIR_NAME))
}

pub fn ensure_conversations_dir() -> AgentJaxResult<PathBuf> {
    let dir = conversations_dir_path()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| AgentJaxError::internal(format!("Failed to create conversations dir {}: {e}", dir.display())).with_error_source(&e))?;
    }
    Ok(dir)
}

pub fn conversation_dir_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    let dir = ensure_conversations_dir()?;
    let safe = sanitize_conversation_id(conversation_id);
    Ok(dir.join(safe))
}

pub fn conversation_metadata_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(conversation_id)?.join(METADATA_FILE_NAME))
}

pub fn conversation_messages_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(conversation_id)?.join(MESSAGES_FILE_NAME))
}

pub fn conversation_workspace_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(conversation_id)?.join(WORKSPACE_DIR_NAME))
}

pub fn ensure_session_layout(conversation_id: &str) -> AgentJaxResult<()> {
    let workspace_dir = conversation_workspace_path(conversation_id)?;
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

pub fn conversation_lcm_db_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    Ok(conversation_dir_path(conversation_id)?.join(LCM_DB_FILE_NAME))
}

pub fn list_conversation_ids() -> AgentJaxResult<Vec<String>> {
    let dir = ensure_conversations_dir()?;
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
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if !name.trim().is_empty() {
                out.push(name.to_string());
            }
        }
    }

    Ok(out)
}
