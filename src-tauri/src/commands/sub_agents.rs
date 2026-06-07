//! Tauri IPC commands for sub-agent management.

use crate::error::AgentJaxError;
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
pub fn cancel_sub_agent(req: CancelSubAgentRequest) -> Result<serde_json::Value, AgentJaxError> {
    SubAgentManager::cancel(&req.agent_id, req.conversation_id.as_deref()).map_err(AgentJaxError::internal)
}

/// List all sub-agents for a conversation.
#[tauri::command]
pub fn list_sub_agents(req: ListSubAgentsRequest) -> Result<Vec<SubAgentSnapshot>, AgentJaxError> {
    Ok(SubAgentManager::list(req.conversation_id.as_deref()))
}
