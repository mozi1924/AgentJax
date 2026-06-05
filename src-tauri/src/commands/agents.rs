//! Tauri commands for agent profile management.
//!
//! Provides CRUD operations over agent profiles, letting the frontend list,
//! create, and delete agents, as well as switch the currently active agent.

use crate::config::agent_config::{AgentConfig, AgentRegistry};
use crate::config::constants::DEFAULT_AGENT_ID;
use serde::{Deserialize, Serialize};
use tauri::State;

// ── Shared state ──────────────────────────────────────────────────────────────

/// Wrapped AgentRegistry for Tauri state management.
pub struct AgentRegistryState {
    registry: AgentRegistry,
}

impl AgentRegistryState {
    pub fn new() -> Result<Self, String> {
        let registry = AgentRegistry::new().map_err(|e| e.to_string())?;
        Ok(Self { registry })
    }
}

// ── DTOs ──────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    pub id: String,
    pub label: String,
}

impl From<&str> for AgentSummary {
    fn from(id: &str) -> Self {
        // Use a human-friendly label derived from the agent ID.
        let label = id
            .replace('-', " ")
            .replace('_', " ")
            .split(' ')
            .map(|word| {
                let mut c = word.chars();
                match c.next() {
                    None => String::new(),
                    Some(first) => first.to_uppercase().chain(c).collect(),
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        Self {
            id: id.to_string(),
            label,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateAgentRequest {
    pub agent_id: String,
    /// Optional: copy settings from an existing agent. Defaults to a fresh config.
    pub template_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteAgentRequest {
    pub agent_id: String,
}

// ── Commands ──────────────────────────────────────────────────────────────────

/// List all discovered agent profiles.
#[tauri::command]
pub fn list_agents(state: State<'_, AgentRegistryState>) -> Result<Vec<AgentSummary>, String> {
    let agent_ids = state.registry.list_agents().map_err(|e| e.to_string())?;
    Ok(agent_ids.iter().map(|id| AgentSummary::from(id.as_str())).collect())
}

/// Create a new agent profile.
///
/// If `template_id` is provided, copies settings from that agent.
/// Otherwise creates a profile with default settings.
#[tauri::command]
pub fn create_agent(
    state: State<'_, AgentRegistryState>,
    req: CreateAgentRequest,
) -> Result<(), String> {
    let agent_id = req.agent_id.trim().to_lowercase();
    if agent_id.is_empty() {
        return Err("Agent ID cannot be empty".to_string());
    }
    if agent_id == DEFAULT_AGENT_ID {
        return Err(format!("Cannot create reserved agent '{}'", DEFAULT_AGENT_ID));
    }

    let config = if let Some(template) = &req.template_id {
        // Clone from an existing agent's config
        let template_config = state.registry.load_agent_config(template).map_err(|e| e.to_string())?;
        template_config
    } else {
        AgentConfig::default()
    };

    state
        .registry
        .create_agent(&agent_id, &config)
        .map_err(|e| e.to_string())?;

    Ok(())
}

/// Delete an agent profile and all its data.
///
/// Cannot delete the default "main" agent.
#[tauri::command]
pub fn delete_agent(
    state: State<'_, AgentRegistryState>,
    req: DeleteAgentRequest,
) -> Result<bool, String> {
    let agent_id = req.agent_id.trim().to_lowercase();
    if agent_id == DEFAULT_AGENT_ID {
        return Err(format!("Cannot delete the default agent '{}'", DEFAULT_AGENT_ID));
    }

    state
        .registry
        .delete_agent(&agent_id)
        .map_err(|e| e.to_string())
}

/// Get the config for a specific agent profile.
#[tauri::command]
pub fn get_agent_config(
    state: State<'_, AgentRegistryState>,
    agent_id: String,
) -> Result<AgentConfig, String> {
    state
        .registry
        .load_agent_config(&agent_id)
        .map_err(|e| e.to_string())
}
