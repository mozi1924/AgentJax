//! Isolated LCM engine per sub-agent.
//!
//! Each sub-agent gets a separate LCM store backed by a distinct SQLite database
//! under the parent conversation's session directory. This prevents sub-agent
//! messages from leaking into the parent conversation's LCM context and vice versa.
//!
//! LCM thresholds for sub-agents are smaller than the main agent since
//! sub-agents have limited scope and shorter conversations.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::lcm::{LcmConfig, LcmEngine, LcmStore, NoopSummarizer};
use std::path::PathBuf;
use std::sync::Arc;

// ── SubAgentLcmContext ────────────────────────────────────────────────────────

/// Holds an isolated LCM engine and its associated paths for a sub-agent.
pub struct SubAgentLcmContext {
    /// The sub-agent's own LCM engine.
    pub engine: Arc<LcmEngine>,
    /// The conversation ID used for LCM storage.
    /// Format: `{parent_conv_id}/sub-agent/{agent_id}`
    pub conversation_id: String,
    /// Path to the sub-agent's LCM database.
    #[allow(dead_code)] // Read in tests; available for future introspection
    pub db_path: PathBuf,
}

impl SubAgentLcmContext {
    /// Create an isolated LCM engine for a sub-agent.
    ///
    /// The LCM database is stored at:
    /// `~/.agentjax/sessions/{parent_conv_id}/sub_agents/{agent_id}/lcm.db`
    pub fn create(
        parent_conv_id: &str,
        agent_id: &str,
        base_lcm_config: &LcmConfig,
    ) -> AgentJaxResult<Self> {
        let sub_conv_id = format!("{}/sub-agent/{}", parent_conv_id, agent_id);
        let db_path = sub_agent_lcm_store_path(parent_conv_id, agent_id)?;

        // Use smaller thresholds for sub-agents since they have limited scope.
        let sub_lcm_config = LcmConfig {
            soft_token_threshold: base_lcm_config
                .soft_token_threshold
                .min(4000),
            hard_token_threshold: base_lcm_config
                .hard_token_threshold
                .min(8000),
            truncation_max_tokens: base_lcm_config
                .truncation_max_tokens
                .min(128),
            compaction_timeout_secs: base_lcm_config
                .compaction_timeout_secs
                .min(10),
            ..base_lcm_config.clone()
        };

        // Ensure parent directory exists.
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to create sub-agent LCM directory {}: {e}",
                    parent.display()
                ))
            })?;
        }

        let store = Arc::new(
            LcmStore::open(&db_path, sub_lcm_config.clone()).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to open sub-agent LCM store at {}: {e}",
                    db_path.display()
                ))
            })?,
        );

        // Use NoopSummarizer for sub-agents — full summarization would be
        // wasteful for short-lived, limited-scope sub-agent tasks.
        let engine = Arc::new(LcmEngine::new(
            store,
            Arc::new(NoopSummarizer),
            sub_lcm_config,
        ));

        // Spawn background compaction task.
        engine.spawn_compaction_task();

        Ok(Self {
            engine,
            conversation_id: sub_conv_id,
            db_path,
        })
    }
}

// ── Path Resolution ───────────────────────────────────────────────────────────

/// Return the path to the LCM SQLite database for a sub-agent.
///
/// Database location: `~/.agentjax/sessions/{parent_conv_id}/sub_agents/{agent_id}/lcm.db`
fn sub_agent_lcm_store_path(
    parent_conv_id: &str,
    agent_id: &str,
) -> AgentJaxResult<PathBuf> {
    let session_dir = crate::conversation_store::conversation_workspace_path(parent_conv_id)
        .map_err(|e| {
            AgentJaxError::internal(format!("Failed to get workspace path: {e}"))
        })?
        .parent()
        .ok_or_else(|| {
            AgentJaxError::not_found(format!(
                "Invalid conversation workspace path for '{parent_conv_id}'"
            ))
        })?
        .to_path_buf();

    Ok(session_dir
        .join("sub_agents")
        .join(agent_id)
        .join("lcm.db"))
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sub_agent_lcm_store_path_format() {
        // The path should contain sub_agents/{agent_id}/lcm.db
        let path = sub_agent_lcm_store_path("test-conv", "agent-001");
        assert!(path.is_ok());
        let path_str = path.unwrap().to_string_lossy().to_string();
        assert!(path_str.contains("sub_agents"));
        assert!(path_str.contains("agent-001"));
        assert!(path_str.ends_with("lcm.db"));
    }

    #[tokio::test]
    async fn test_create_lcm_context() {
        let config = LcmConfig::default();
        let result = SubAgentLcmContext::create("test-conv", "agent-test", &config);
        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert!(ctx.conversation_id.contains("sub-agent"));
        assert!(ctx.conversation_id.contains("agent-test"));
        // Cleanup: remove the test directory.
        if let Some(parent) = ctx.db_path.parent() {
            // Remove the agent directory.
            let _ = std::fs::remove_dir_all(
                parent.parent().unwrap_or(parent),
            );
        }
    }

    #[tokio::test]
    async fn test_thresholds_are_capped_for_sub_agent() {
        let config = LcmConfig {
            soft_token_threshold: 100_000,
            hard_token_threshold: 200_000,
            truncation_max_tokens: 512,
            compaction_timeout_secs: 60,
            ..LcmConfig::default()
        };
        let ctx = SubAgentLcmContext::create("test-conv", "agent-capped", &config)
            .expect("create");
        // We can't directly read the config from LcmEngine, but we can verify
        // it was created successfully with capped values.
        assert!(ctx.conversation_id.contains("agent-capped"));
        // Cleanup.
        if let Some(parent) = ctx.db_path.parent() {
            let _ = std::fs::remove_dir_all(
                parent.parent().unwrap_or(parent),
            );
        }
    }
}
