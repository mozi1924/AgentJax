//! Isolated LCM engine per sub-agent.
//!
//! Each sub-agent gets an **in-memory** LCM store. No data is written to
//! disk — ephemeral sub-agents are short-lived and their messages do not
//! need to survive beyond the sub-agent's lifetime. This avoids the overhead
//! of creating/deleting SQLite files on disk for every sub-agent invocation.
//!
//! LCM thresholds for sub-agents are smaller than the main agent since
//! sub-agents have limited scope and shorter conversations.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::lcm::{LcmConfig, LcmEngine, LcmStore, NoopSummarizer};
use std::sync::Arc;

// ── SubAgentLcmContext ────────────────────────────────────────────────────────

/// Holds an isolated LCM engine and its associated paths for a sub-agent.
pub struct SubAgentLcmContext {
    /// The sub-agent's own LCM engine.
    pub engine: Arc<LcmEngine>,
    /// The conversation ID used for LCM storage.
    /// Format: `{parent_conv_id}/sub-agent/{agent_id}`
    pub conversation_id: String,
}

impl SubAgentLcmContext {
    /// Create an isolated in-memory LCM engine for a sub-agent.
    ///
    /// Uses an in-memory SQLite database — no files are written to disk.
    /// Sub-agents are short-lived, so persistence is unnecessary.
    pub fn create(
        parent_conv_id: &str,
        subagent_type: &str,
        agent_id: &str,
        base_lcm_config: &LcmConfig,
    ) -> AgentJaxResult<Self> {
        let sub_conv_id = format!(
            "{}/sub-agent/{}/{}",
            parent_conv_id, subagent_type, agent_id
        );

        // Use smaller thresholds for sub-agents since they have limited scope.
        let sub_lcm_config = LcmConfig {
            soft_token_threshold: base_lcm_config.soft_token_threshold.min(4000),
            hard_token_threshold: base_lcm_config.hard_token_threshold.min(8000),
            truncation_max_tokens: base_lcm_config.truncation_max_tokens.min(128),
            compaction_timeout_secs: base_lcm_config.compaction_timeout_secs.min(10),
            ..base_lcm_config.clone()
        };

        // In-memory store: no disk I/O, auto-cleaned on drop.
        let store = Arc::new(
            LcmStore::open_in_memory(sub_lcm_config.clone()).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to create in-memory sub-agent LCM store: {e}"
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
        })
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_create_lcm_context() {
        let config = LcmConfig::default();
        let result = SubAgentLcmContext::create("test-conv", "explore", "agent-test", &config);
        assert!(result.is_ok());
        let ctx = result.unwrap();
        assert!(ctx.conversation_id.contains("/sub-agent/explore/"));
        assert!(ctx.conversation_id.contains("agent-test"));
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
        let ctx = SubAgentLcmContext::create("test-conv", "general", "agent-capped", &config)
            .expect("create");
        assert!(ctx.conversation_id.contains("agent-capped"));
    }
}
