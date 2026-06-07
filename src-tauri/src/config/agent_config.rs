//! Agent Configuration — per-agent settings isolated in `~/.agentjax/agents/{id}/agent.yaml`.
//!
//! Each agent profile has its own `agent.yaml` that holds the agent-specific
//! configuration: which model to use, tool availability, prompt composer blocks,
//! memory & RAG settings, sub-agent limits, and context management tuning.
//!
//! The shared `config.yaml` at the root holds provider credentials, MCP server
//! definitions, and plugin manager settings that are shared across all agents.

use crate::config::constants::{AGENT_CONFIG_FILE_NAME, DEFAULT_TIMEOUT_SECONDS};
use crate::config::prompt_composer::{PromptComposerConfig, normalize_prompt_composer};
use crate::config::schema::{
    ContextManagementConfig, MemoryConfig, SubAgentConfig, ToolManagerConfig,
};
use crate::error::{AgentJaxError, AgentJaxResult};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

// ── AgentId type alias ─────────────────────────────────────────────────────────

/// A unique identifier for an agent profile.
///
/// Used as both the directory name under `~/.agentjax/agents/` and the
/// logical key for referencing the agent in API calls.
pub type AgentId = String;

// ── AgentConfig ────────────────────────────────────────────────────────────────

/// Per-agent configuration, stored in `agents/{agent_id}/agent.yaml`.
///
/// These fields were historically part of `AppConfig` but are now scoped
/// to individual agent profiles so that different agents (e.g. "main" for
/// daily life, "coding" for work) can have fully independent settings.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(default)]
pub struct AgentConfig {
    /// The active provider key (must match a key in AppConfig.providers).
    pub active_provider: String,
    /// The default model reference: `{provider}/{model_id}`.
    pub default_model: String,
    /// A smaller/cheaper model for utility tasks (summarization, etc).
    pub utility_small_model: String,
    /// Default timeout for API requests (seconds).
    pub request_timeout_seconds: u64,
    /// Maximum number of tool execution turns (hops) allowed per request (0 for unlimited).
    pub max_tool_turns: usize,
    /// Prompt composer — the assembly of system/developer prompt blocks.
    #[serde(default)]
    pub prompt_composer: PromptComposerConfig,
    /// Context management (LCM + Street) settings.
    #[serde(default)]
    pub context_management: ContextManagementConfig,
    /// Sub-agent concurrency and limits.
    #[serde(default)]
    pub sub_agent: SubAgentConfig,
    /// Memory system settings.
    #[serde(default)]
    pub memory: MemoryConfig,
    /// Tool management — enable/disable individual tools per agent.
    #[serde(default)]
    pub tool_manager: ToolManagerConfig,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            active_provider: String::new(),
            default_model: String::new(),
            utility_small_model: String::new(),
            request_timeout_seconds: DEFAULT_TIMEOUT_SECONDS,
            max_tool_turns: 0,
            prompt_composer: PromptComposerConfig::default(),
            context_management: ContextManagementConfig::default(),
            sub_agent: SubAgentConfig::default(),
            memory: MemoryConfig::default(),
            tool_manager: ToolManagerConfig::default(),
        }
    }
}

impl AgentConfig {
    /// Normalize all agent-specific fields (trim whitespace, lowercase keys, etc).
    pub fn normalize(mut self) -> Self {
        self.active_provider = self.active_provider.trim().to_lowercase();
        self.default_model = self.default_model.trim().to_string();
        self.utility_small_model = self.utility_small_model.trim().to_string();
        if self.utility_small_model.is_empty() {
            self.utility_small_model = self.default_model.clone();
        }
        if self.request_timeout_seconds == 0 {
            self.request_timeout_seconds = DEFAULT_TIMEOUT_SECONDS;
        }
        self.prompt_composer = normalize_prompt_composer(self.prompt_composer);
        self.tool_manager = self.tool_manager.normalize();
        self
    }

    /// Merge fields from a default `AgentConfig`, preferring non-empty existing values.
    /// Used when upgrading an existing agent.yaml with new fields.
    pub fn merge_defaults(mut self, defaults: &Self) -> Self {
        if self.active_provider.is_empty() {
            self.active_provider = defaults.active_provider.clone();
        }
        if self.default_model.is_empty() {
            self.default_model = defaults.default_model.clone();
        }
        if self.utility_small_model.is_empty() {
            self.utility_small_model = defaults.utility_small_model.clone();
        }
        self
    }
}

// ── Agent Registry ─────────────────────────────────────────────────────────────

/// A registry that discovers and manages agent profiles on disk.
///
/// Agents are discovered by scanning `~/.agentjax/agents/` for subdirectories
/// that contain an `agent.yaml` file. The registry provides CRUD operations
/// for managing agent profiles.
#[derive(Debug, Clone)]
pub struct AgentRegistry {
    agents_dir: PathBuf,
}

impl AgentRegistry {
    /// Create a new registry rooted at `~/.agentjax/agents/`.
    pub fn new() -> AgentJaxResult<Self> {
        let agents_dir = crate::agentjax_home::agents_dir()?;
        Ok(Self { agents_dir })
    }

    /// Create a registry from a specific agents directory (for testing).
    pub fn with_dir(agents_dir: PathBuf) -> Self {
        Self { agents_dir }
    }

    /// List all discovered agent IDs.
    pub fn list_agents(&self) -> AgentJaxResult<Vec<AgentId>> {
        if !self.agents_dir.exists() {
            return Ok(Vec::new());
        }

        let mut agents = Vec::new();
        let entries = std::fs::read_dir(&self.agents_dir).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to read agents directory {}: {e}",
                self.agents_dir.display()
            ))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                AgentJaxError::config(format!("Failed to inspect agent directory entry: {e}"))
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
                // Only consider directories that contain an agent.yaml
                if path.join(AGENT_CONFIG_FILE_NAME).exists() {
                    agents.push(name.to_string());
                }
            }
        }

        agents.sort();
        Ok(agents)
    }

    /// Check if an agent with the given ID exists.
    pub fn agent_exists(&self, agent_id: &str) -> bool {
        self.agent_dir(agent_id)
            .join(AGENT_CONFIG_FILE_NAME)
            .exists()
    }

    /// Get the directory for a specific agent.
    pub fn agent_dir(&self, agent_id: &str) -> PathBuf {
        self.agents_dir.join(sanitize_agent_id(agent_id))
    }

    /// Get the path to an agent's config file.
    pub fn agent_config_path(&self, agent_id: &str) -> PathBuf {
        self.agent_dir(agent_id).join(AGENT_CONFIG_FILE_NAME)
    }

    /// Load an agent's configuration from its `agent.yaml`.
    pub fn load_agent_config(&self, agent_id: &str) -> AgentJaxResult<AgentConfig> {
        let path = self.agent_config_path(agent_id);
        if !path.exists() {
            return Err(AgentJaxError::not_found(format!(
                "Agent config not found for '{}' at {}",
                agent_id,
                path.display()
            )));
        }
        let raw = std::fs::read_to_string(&path).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to read agent config for '{}' at {}: {e}",
                agent_id,
                path.display()
            ))
        })?;
        let config: AgentConfig = serde_yaml::from_str(&raw).map_err(|e| {
            AgentJaxError::config(format!(
                "Invalid YAML in agent config for '{}' at {}: {e}",
                agent_id,
                path.display()
            ))
        })?;
        Ok(config.normalize())
    }

    /// Write an agent's configuration to its `agent.yaml`.
    pub fn write_agent_config(&self, agent_id: &str, config: &AgentConfig) -> AgentJaxResult<()> {
        let dir = self.agent_dir(agent_id);
        std::fs::create_dir_all(&dir).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to create agent directory for '{}' at {}: {e}",
                agent_id,
                dir.display()
            ))
        })?;

        let path = dir.join(AGENT_CONFIG_FILE_NAME);
        let yaml = serde_yaml::to_string(config).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to serialize agent config for '{}': {e}",
                agent_id
            ))
        })?;
        std::fs::write(&path, yaml).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to write agent config for '{}' at {}: {e}",
                agent_id,
                path.display()
            ))
        })?;
        Ok(())
    }

    /// Create a new agent profile with the given config.
    /// Returns an error if the agent already exists.
    pub fn create_agent(&self, agent_id: &str, config: &AgentConfig) -> AgentJaxResult<()> {
        if self.agent_exists(agent_id) {
            return Err(AgentJaxError::config(format!(
                "Agent '{}' already exists",
                agent_id
            )));
        }
        self.write_agent_config(agent_id, config)
    }

    /// Delete an agent profile and all its data.
    pub fn delete_agent(&self, agent_id: &str) -> AgentJaxResult<bool> {
        let dir = self.agent_dir(agent_id);
        if !dir.exists() {
            return Ok(false);
        }
        std::fs::remove_dir_all(&dir).map_err(|e| {
            AgentJaxError::config(format!(
                "Failed to delete agent '{}' at {}: {e}",
                agent_id,
                dir.display()
            ))
        })?;
        Ok(true)
    }
}

// ── Helpers ────────────────────────────────────────────────────────────────────

/// Default agent config used when creating a new agent profile (test helper).
#[cfg(test)]
pub fn default_agent_config() -> AgentConfig {
    AgentConfig::default()
}

/// Sanitize an agent ID for use as a directory name.
/// Only allows lowercase alphanumeric, hyphens, and underscores.
fn sanitize_agent_id(agent_id: &str) -> String {
    agent_id
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>()
        .to_lowercase()
}

// ── FullConfig ─────────────────────────────────────────────────────────────────

/// The merged view of shared config + agent-specific config.
///
/// This is what the runtime actually uses: `AppConfig` provides providers,
/// MCP servers, and plugin managers that are shared across agents, while
/// `AgentConfig` provides the per-agent settings.
#[derive(Debug, Clone)]
pub struct FullConfig {
    /// Shared configuration (from `~/.agentjax/config.yaml`).
    pub shared: crate::config::AppConfig,
    /// Agent-specific configuration (from `~/.agentjax/agents/{id}/agent.yaml`).
    pub agent: AgentConfig,
    /// The agent ID this config is for.
    pub agent_id: AgentId,
}

impl FullConfig {
    /// Create a new FullConfig from its parts.
    pub fn new(shared: crate::config::AppConfig, agent: AgentConfig, agent_id: AgentId) -> Self {
        Self {
            shared,
            agent,
            agent_id,
        }
    }

    /// Convenience: get the active provider key (from agent config).
    pub fn active_provider(&self) -> &str {
        &self.agent.active_provider
    }

    /// Convenience: get the default model reference.
    pub fn default_model(&self) -> &str {
        &self.agent.default_model
    }

    /// Convenience: get the utility small model reference.
    pub fn utility_small_model(&self) -> &str {
        &self.agent.utility_small_model
    }

    /// Convenience: get providers from the shared config.
    pub fn providers(&self) -> &std::collections::BTreeMap<String, crate::config::ProviderConfig> {
        &self.shared.providers
    }

    /// Convenience: get the request timeout (from agent config).
    pub fn request_timeout_seconds(&self) -> u64 {
        self.agent.request_timeout_seconds
    }

    /// Resolve a model profile across both config layers.
    ///
    /// Uses the agent's model references (default_model, utility_small_model)
    /// but resolves providers from the shared AppConfig.providers.
    pub fn resolve_model_profile(
        &self,
        requested: Option<&str>,
    ) -> AgentJaxResult<crate::config::ResolvedModelConfig> {
        // Delegate to AppConfig's resolve method using agent config defaults.
        self.shared
            .resolve_model_profile_with_agent(requested, &self.agent)
    }
}

// ── Tests ──────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sanitize_agent_id() {
        assert_eq!(sanitize_agent_id("main"), "main");
        assert_eq!(sanitize_agent_id("My Agent!"), "myagent");
        assert_eq!(sanitize_agent_id("coding-work"), "coding-work");
        assert_eq!(sanitize_agent_id("UPPER_CASE"), "upper_case");
    }

    #[test]
    fn test_agent_config_default_normalize() {
        let config = AgentConfig::default().normalize();
        assert_eq!(config.request_timeout_seconds, DEFAULT_TIMEOUT_SECONDS);
    }

    #[test]
    fn test_agent_registry_no_dir() {
        let tmp = std::env::temp_dir().join(format!("agent-registry-{}", uuid::Uuid::new_v4()));
        let registry = AgentRegistry::with_dir(tmp.clone());
        let agents = registry.list_agents().unwrap();
        assert!(agents.is_empty());
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agent_registry_create_and_list() {
        let tmp = std::env::temp_dir().join(format!("agent-registry-{}", uuid::Uuid::new_v4()));
        let registry = AgentRegistry::with_dir(tmp.clone());

        let config = default_agent_config();
        registry.create_agent("main", &config).unwrap();
        registry.create_agent("coding", &config).unwrap();

        let agents = registry.list_agents().unwrap();
        assert_eq!(agents.len(), 2);
        assert!(agents.contains(&"coding".to_string()));
        assert!(agents.contains(&"main".to_string()));

        assert!(registry.agent_exists("main"));
        assert!(!registry.agent_exists("nonexistent"));

        // Should still appear after reload
        let registry2 = AgentRegistry::with_dir(tmp.clone());
        let agents2 = registry2.list_agents().unwrap();
        assert_eq!(agents2.len(), 2);

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agent_config_read_write_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("agent-rw-{}", uuid::Uuid::new_v4()));
        let registry = AgentRegistry::with_dir(tmp.clone());

        let mut config = default_agent_config();
        config.active_provider = "openai".to_string();
        config.default_model = "openai/gpt-5".to_string();
        config.utility_small_model = "openai/gpt-5-mini".to_string();

        registry.create_agent("test", &config).unwrap();
        let loaded = registry.load_agent_config("test").unwrap();

        assert_eq!(loaded.active_provider, "openai");
        assert_eq!(loaded.default_model, "openai/gpt-5");
        assert_eq!(loaded.utility_small_model, "openai/gpt-5-mini");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agent_delete() {
        let tmp = std::env::temp_dir().join(format!("agent-del-{}", uuid::Uuid::new_v4()));
        let registry = AgentRegistry::with_dir(tmp.clone());

        registry
            .create_agent("temp", &default_agent_config())
            .unwrap();
        assert!(registry.agent_exists("temp"));

        registry.delete_agent("temp").unwrap();
        assert!(!registry.agent_exists("temp"));

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn test_agent_config_merge_defaults() {
        let base = AgentConfig::default();
        let partial = AgentConfig {
            active_provider: String::new(),
            default_model: String::new(),
            utility_small_model: "my/model".to_string(),
            ..AgentConfig::default()
        };

        let merged = partial.merge_defaults(&base);
        // Empty fields should be filled from defaults
        assert_eq!(merged.active_provider, base.active_provider);
        assert_eq!(merged.default_model, base.default_model);
        // Non-empty should be kept
        assert_eq!(merged.utility_small_model, "my/model");
    }
}
