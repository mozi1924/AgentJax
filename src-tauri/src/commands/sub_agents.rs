//! Tauri IPC commands for sub-agent management.

use crate::sub_agents::manager::SubAgentManager;
use crate::sub_agents::types::SubAgentSnapshot;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelSubAgentRequest {
    pub agent_id: String,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListSubAgentsRequest {
    pub conversation_id: Option<String>,
}

/// Cancel a running sub-agent.
#[tauri::command]
pub fn cancel_sub_agent(req: CancelSubAgentRequest) -> Result<serde_json::Value, String> {
    SubAgentManager::cancel(&req.agent_id, req.conversation_id.as_deref()).map_err(|e| e.to_string())
}

/// List all sub-agents for a conversation.
#[tauri::command]
pub fn list_sub_agents(req: ListSubAgentsRequest) -> Result<Vec<SubAgentSnapshot>, String> {
    Ok(SubAgentManager::list(req.conversation_id.as_deref()))
}
