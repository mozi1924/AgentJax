use crate::conversation_store_utils::sanitize_conversation_id;
use std::fs;
use std::path::PathBuf;

const AGENTJAX_DIR_NAME: &str = ".agentjax";
const SESSIONS_DIR_NAME: &str = "sessions";
const METADATA_FILE_NAME: &str = "metadata.json";
const MESSAGES_FILE_NAME: &str = "messages.jsonl";
const WORKSPACE_DIR_NAME: &str = "workspace";

pub fn agentjax_home_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir()
        .ok_or_else(|| "Failed to resolve home directory for .agentjax".to_string())?;
    Ok(home.join(AGENTJAX_DIR_NAME))
}

pub fn conversations_dir_path() -> Result<PathBuf, String> {
    Ok(agentjax_home_dir()?.join(SESSIONS_DIR_NAME))
}

pub fn ensure_conversations_dir() -> Result<PathBuf, String> {
    let dir = conversations_dir_path()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create conversations dir {}: {e}", dir.display()))?;
    }
    Ok(dir)
}

pub fn conversation_dir_path(conversation_id: &str) -> Result<PathBuf, String> {
    let dir = ensure_conversations_dir()?;
    let safe = sanitize_conversation_id(conversation_id);
    Ok(dir.join(safe))
}

pub fn conversation_metadata_path(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversation_dir_path(conversation_id)?.join(METADATA_FILE_NAME))
}

pub fn conversation_messages_path(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversation_dir_path(conversation_id)?.join(MESSAGES_FILE_NAME))
}

pub fn conversation_workspace_path(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversation_dir_path(conversation_id)?.join(WORKSPACE_DIR_NAME))
}

pub fn ensure_session_layout(conversation_id: &str) -> Result<(), String> {
    let workspace_dir = conversation_workspace_path(conversation_id)?;
    if !workspace_dir.exists() {
        fs::create_dir_all(&workspace_dir).map_err(|e| {
            format!(
                "Failed to create session workspace directory {}: {e}",
                workspace_dir.display()
            )
        })?;
    }
    Ok(())
}

pub fn list_conversation_ids() -> Result<Vec<String>, String> {
    let dir = ensure_conversations_dir()?;
    let mut out = Vec::new();

    let entries = fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read conversations dir {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to inspect conversation file entry: {e}"))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }
        let metadata = path.join(METADATA_FILE_NAME);
        let messages = path.join(MESSAGES_FILE_NAME);
        if !metadata.exists() || !messages.exists() {
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
