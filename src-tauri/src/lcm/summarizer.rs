//! Provider-backed summarizer for LCM compaction.
//!
//! Replaces the `NoopSummarizer` with a real LLM-powered implementation
//! that uses the framework's provider API. Uses the configured
//! `summarization_model` (or the app's `utility_small_model` as fallback)
//! to generate summaries during Level 1 and Level 2 compaction.

use crate::lcm::compaction::Summarizer;
use crate::lcm::types::LcmError;
use crate::provider_api::retry::{RetryStrategy, retry_with_backoff};
use crate::provider_api::types::ResponseStreamRequest;
use tokio::sync::watch;

/// System prompt for the summarization model.
const SUMMARIZE_SYSTEM_PROMPT: &str = "\
You are a precise conversation summarizer for a context management system. \
Your summaries are used to compress older messages while preserving \
lossless retrievability of the originals.

Guidelines:
- Preserve key facts, decisions, code patterns, file paths, and architectural choices.
- Keep user questions intact in condensed form.
- Mention tool calls by name and their outcomes.
- Use the same language as the input.
- Be concise but complete — do not drop critical information.
- Output ONLY the summary text — no preamble, no markdown headers, no commentary.";

/// A `Summarizer` implementation that uses the framework's provider API.
///
/// Makes real LLM API calls for Level 1 ("preserve_details") and
/// Level 2 ("bullet_points") summarization modes.
pub struct ProviderSummarizer {
    /// The app configuration (for provider/model resolution).
    config: crate::config::AppConfig,
    /// The agent configuration (for agent-specific model defaults).
    agent_config: crate::config::AgentConfig,
    /// The model reference to use for summarization.
    model_ref: String,
}

impl ProviderSummarizer {
    /// Create a new provider-backed summarizer.
    ///
    /// `lcm_config` provides the `summarization_model` override.
    /// If empty or "default", the agent's `utility_small_model` is used.
    pub fn new(
        app_config: &crate::config::AppConfig,
        agent_config: &crate::config::AgentConfig,
        lcm_config: &crate::lcm::types::LcmConfig,
    ) -> Result<Self, LcmError> {
        let model_ref = if lcm_config.summarization_model.is_empty()
            || lcm_config.summarization_model == "default"
        {
            agent_config.utility_small_model.clone()
        } else {
            lcm_config.summarization_model.clone()
        };

        if model_ref.is_empty() {
            return Err(LcmError::Config(
                "No summarization model configured. Set lcm.summarization_model or configure a utility_small_model.".to_string(),
            ));
        }

        // Validate the model reference resolves.
        let _resolved = app_config
            .resolve_model_profile_with_agent(Some(&model_ref), agent_config)
            .map_err(|e| {
                LcmError::Config(format!(
                    "Failed to resolve summarization model '{}': {}",
                    model_ref, e
                ))
            })?;

        Ok(Self {
            config: app_config.clone(),
            agent_config: agent_config.clone(),
            model_ref,
        })
    }

    /// Returns the model reference being used for summarization.
    pub fn model_ref(&self) -> &str {
        &self.model_ref
    }

    /// Build the summarization prompt for a given mode.
    fn build_prompt(content: &str, mode: &str) -> String {
        match mode {
            "preserve_details" => format!(
                "Summarize the following conversation segment while preserving \
                 all key details, decisions, code patterns, and file paths. \
                 Keep it concise but complete.\n\n---\n\n{content}\n\n---\n\nSummary:"
            ),
            "bullet_points" => format!(
                "Create a bullet-point summary of the following conversation segment. \
                 Focus on the most important facts, actions, and decisions. \
                 Be very concise — use short lines.\n\n---\n\n{content}\n\n---\n\nBullet-point summary:"
            ),
            _ => format!(
                "Summarize the following conversation segment:\n\n---\n\n{content}\n\n---\n\nSummary:"
            ),
        }
    }
}

#[async_trait::async_trait]
impl Summarizer for ProviderSummarizer {
    async fn summarize(
        &self,
        content: &str,
        mode: &str,
        _target_tokens: u32,
    ) -> Result<String, LcmError> {
        let prompt = Self::build_prompt(content, mode);

        let request = ResponseStreamRequest {
            input_items: vec![serde_json::json!({
                "role": "user",
                "content": [{
                    "type": "input_text",
                    "text": prompt
                }]
            })],
            model: Some(self.model_ref.clone()),
            reasoning: None,
            instructions_override: Some(SUMMARIZE_SYSTEM_PROMPT.to_string()),
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: None,
            tool_choice: None,
            skip_model_extra_body: true,
            ..Default::default()
        };

        // ── Retry with backoff on empty/incomplete responses ──
        let config = &self.config;
        let response = retry_with_backoff(RetryStrategy::empty_response(), || async {
            let mut cancel_rx = watch::channel(false).1;
            let result = crate::provider_api::stream_response(
                config,
                &self.agent_config,
                &request,
                &mut cancel_rx,
                |_| Ok(()),
            )
            .await;

            // If the response is empty or very short, treat as retryable.
            match &result {
                Ok(res) if res.output_text.trim().len() < 10 => {
                    Err(crate::error::AgentJaxError::internal(format!(
                        "Summarization returned empty/short response ({} chars)",
                        res.output_text.len()
                    )))
                }
                _ => result.map_err(|e| {
                    crate::error::AgentJaxError::internal(format!(
                        "Summarization API call failed: {e}"
                    ))
                }),
            }
        })
        .await
        .into_result()
        .map_err(|e| LcmError::Compaction(format!("Summarization failed after retry: {e}")))?;

        let summary = response.output_text.trim().to_string();

        if summary.is_empty() {
            return Err(LcmError::Compaction(
                "Summarization returned empty response after retry".to_string(),
            ));
        }

        Ok(summary)
    }
}
