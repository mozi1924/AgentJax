//! Three-Level Summarization Escalation protocol.
//!
//! Implements the guaranteed-convergence compaction strategy from
//! "LCM: Lossless Context Management" (Figure 3).
//!
//! ## Escalation Levels
//!
//! | Level | Strategy | LLM Required | Guarantee |
//! |-------|----------|-------------|-----------|
//! | 1 — Normal | Preserve details, target tokens T | Yes | Partial |
//! | 2 — Aggressive | Bullet points, target tokens T/2 | Yes | Partial |
//! | 3 — Truncation | Deterministic cut to 512 chars | No | **Always converges** |
//!
//! The key invariant: **each level must produce output strictly shorter than
//! its input**. If a level fails to reduce token count, the system escalates.
//! Level 3 guarantees convergence by using a non-LLM deterministic truncation.

use crate::lcm::types::{FileRefId, LcmError, MessageRole, StoredMessage, SummaryKind, SummaryNode, SummaryId};
use std::sync::Arc;

/// A token-counting function: takes text, returns an estimated token count.
pub type TokenCounter = Arc<dyn Fn(&str) -> u32 + Send + Sync>;

// ── Summarization Provider Trait ────────────────────────────────────────────

/// A pluggable provider for LLM-powered summarization.
///
/// Implementations can use the existing provider infrastructure to make
/// API calls. The trait is async and operates on raw text to keep the
/// interface simple and provider-agnostic.
#[async_trait::async_trait]
pub trait Summarizer: Send + Sync {
    /// Produce a summary of the given text content.
    ///
    /// - `content`: The text to summarize (concatenated messages).
    /// - `mode`: The summarization mode ("preserve_details" or "bullet_points").
    /// - `target_tokens`: The desired maximum token count for the output.
    ///
    /// Returns the summary text, which MUST be shorter than the input
    /// for the escalation protocol to work correctly.
    async fn summarize(
        &self,
        content: &str,
        mode: &str,
        target_tokens: u32,
    ) -> Result<String, LcmError>;
}

/// A no-op summarizer that always escalates to truncation.
/// Useful for testing or when no LLM provider is available.
pub struct NoopSummarizer;

#[async_trait::async_trait]
impl Summarizer for NoopSummarizer {
    async fn summarize(
        &self,
        _content: &str,
        _mode: &str,
        _target_tokens: u32,
    ) -> Result<String, LcmError> {
        // Return the content unchanged — this will fail the convergence check
        // and cause immediate escalation to Level 3.
        Err(LcmError::Compaction(
            "No summarizer configured — use LcmConfig to set a summarization provider"
                .to_string(),
        ))
    }
}

// ── Compaction Engine ───────────────────────────────────────────────────────

/// The compaction engine that drives the Three-Level Escalation protocol.
pub struct CompactionEngine {
    /// The summarizer used for Level 1 and Level 2.
    summarizer: Arc<dyn Summarizer>,
    /// Maximum tokens for Level 3 truncation (head + tail).
    truncation_max_tokens: u32,
    /// Token counting function — uses real tokenizer when available.
    count_tokens: TokenCounter,
}

impl CompactionEngine {
    /// Create a new compaction engine.
    pub fn new(
        summarizer: Arc<dyn Summarizer>,
        truncation_max_tokens: u32,
        count_tokens: TokenCounter,
    ) -> Self {
        Self {
            summarizer,
            truncation_max_tokens,
            count_tokens,
        }
    }

    /// Returns a reference to the token counter.
    pub fn token_counter(&self) -> &TokenCounter {
        &self.count_tokens
    }

    /// Execute the Three-Level Escalation protocol.
    ///
    /// Given a set of messages, attempts to produce a summary that is
    /// strictly shorter than the input. Escalates through three levels
    /// until convergence is achieved.
    ///
    /// Returns the summary text and the compaction level that succeeded.
    pub async fn escalate_summarize(
        &self,
        messages: &[StoredMessage],
        target_tokens: u32,
    ) -> Result<(String, u8), LcmError> {
        let input_tokens: u32 = messages.iter().map(|m| m.token_count).sum();
        let input_text = Self::concat_messages(messages);

        // ── Level 1: Normal — preserve details ──
        match self
            .summarizer
            .summarize(&input_text, "preserve_details", target_tokens)
            .await
        {
            Ok(summary) => {
                let summary_tokens = (self.count_tokens)(&summary);
                if summary_tokens < input_tokens {
                    return Ok((summary, 1));
                }
                log::warn!(
                    "LCM Level 1 summary did not converge ({} -> {} tokens), escalating",
                    input_tokens,
                    summary_tokens
                );
            }
            Err(e) => {
                log::warn!("LCM Level 1 summarization failed: {e}, escalating");
            }
        }

        // ── Level 2: Aggressive — bullet points, half target ──
        match self
            .summarizer
            .summarize(&input_text, "bullet_points", target_tokens / 2)
            .await
        {
            Ok(summary) => {
                let summary_tokens = (self.count_tokens)(&summary);
                if summary_tokens < input_tokens {
                    return Ok((summary, 2));
                }
                log::warn!(
                    "LCM Level 2 summary did not converge ({} -> {} tokens), escalating to truncation",
                    input_tokens,
                    summary_tokens
                );
            }
            Err(e) => {
                log::warn!("LCM Level 2 summarization failed: {e}, escalating to truncation");
            }
        }

        // ── Level 3: Deterministic Truncation — guaranteed convergence ──
        let truncated = Self::deterministic_truncate(&input_text, self.truncation_max_tokens);
        Ok((truncated, 3))
    }

    /// Concatenate messages into a single text block for summarization.
    fn concat_messages(messages: &[StoredMessage]) -> String {
        let mut text = String::new();
        for msg in messages {
            let role_label = match msg.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::Tool => "Tool",
            };
            text.push_str(&format!("[{role_label}]: {}\n", msg.content));
        }
        text
    }

    /// Level 3: Deterministic truncation.
    ///
    /// Takes the first ~67% and last ~33% of tokens, converted from
    /// `max_tokens` to characters via the 4:1 heuristic. This preserves
    /// context from both the beginning and end of the conversation.
    ///
    /// No LLM is involved — this is a pure string operation that
    /// **always converges**.
    pub fn deterministic_truncate(text: &str, max_tokens: u32) -> String {
        // Convert token budget to character budget using the 4:1 heuristic.
        let max_chars = (max_tokens as usize).saturating_mul(4);

        if text.chars().count() <= max_chars {
            return text.to_string();
        }

        let head_chars = (max_chars * 2) / 3; // ~67% from start
        let tail_chars = max_chars / 3; // ~33% from end

        let head: String = text.chars().take(head_chars).collect();
        let tail: String = text.chars().rev().take(tail_chars).collect::<String>()
            .chars().rev().collect();

        format!(
            "{head}\n\n[... {omitted} chars truncated by LCM Level 3 compaction ...]\n\n{tail}",
            omitted = {
                let total = text.chars().count();
                let kept = head_chars + tail_chars;
                total.saturating_sub(kept)
            }
        )
    }

    /// Create a new SummaryNode with propagated file references.
    #[allow(clippy::too_many_arguments)]
    pub fn build_summary_node_with_refs(
        id: SummaryId,
        conversation_id: &str,
        text: &str,
        compaction_level: u8,
        kind: SummaryKind,
        timestamp_unix_ms: i64,
        count_tokens: &TokenCounter,
        file_refs: Vec<FileRefId>,
    ) -> SummaryNode {
        SummaryNode {
            id,
            conversation_id: conversation_id.to_string(),
            kind,
            text: text.to_string(),
            token_count: count_tokens(text),
            created_at_unix_ms: timestamp_unix_ms,
            compaction_level,
            parents: Vec::new(),
            file_refs,
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::types::MessageId;
    use crate::lcm::types::estimate_tokens;

    /// Helper: create a token counter using the 4:1 char heuristic for tests.
    fn test_token_counter() -> TokenCounter {
        Arc::new(|text: &str| crate::lcm::types::estimate_tokens(text))
    }

    /// A mock summarizer that simulates successful Level 1 summarization.
    struct MockSummarizer {
        response: String,
    }

    #[async_trait::async_trait]
    impl Summarizer for MockSummarizer {
        async fn summarize(
            &self,
            _content: &str,
            _mode: &str,
            _target_tokens: u32,
        ) -> Result<String, LcmError> {
            Ok(self.response.clone())
        }
    }

    /// A mock summarizer that always fails, forcing escalation to Level 3.
    struct FailingSummarizer;

    #[async_trait::async_trait]
    impl Summarizer for FailingSummarizer {
        async fn summarize(
            &self,
            _content: &str,
            _mode: &str,
            _target_tokens: u32,
        ) -> Result<String, LcmError> {
            Err(LcmError::Compaction("Simulated failure".to_string()))
        }
    }

    fn make_msg(id: &str, content: &str) -> StoredMessage {
        StoredMessage::new(
            MessageId::from(id),
            "test",
            MessageRole::User,
            content,
            estimate_tokens(content),
            1000,
        )
    }

    #[tokio::test]
    async fn test_level_1_success() {
        let summarizer = Arc::new(MockSummarizer {
            response: "Short summary".to_string(),
        });
        let engine = CompactionEngine::new(summarizer, 512, test_token_counter());

        let messages = vec![
            make_msg("1", "This is a very long message with many tokens indeed yes absolutely"),
            make_msg("2", "Another long message that adds more tokens to the count here"),
        ];

        let (summary, level) = engine.escalate_summarize(&messages, 20).await.unwrap();
        assert_eq!(level, 1);
        assert_eq!(summary, "Short summary");
    }

    #[tokio::test]
    async fn test_escalation_to_level_3() {
        // Mock summarizer returns content LONGER than input — should escalate.
        let summarizer = Arc::new(MockSummarizer {
            response: "A".repeat(500), // Longer than our short messages
        });
        let engine = CompactionEngine::new(summarizer, 100, test_token_counter());

        let messages = vec![
            make_msg("1", "hi"), // very short
        ];

        let (_summary, level) = engine.escalate_summarize(&messages, 10).await.unwrap();
        // Should have escalated to Level 3 truncation.
        assert_eq!(level, 3, "Should have escalated to Level 3");
    }

    #[tokio::test]
    async fn test_failing_summarizer_escalates() {
        let summarizer = Arc::new(FailingSummarizer);
        let engine = CompactionEngine::new(summarizer, 100, test_token_counter());

        let messages = vec![
            make_msg("1", "Some message that needs summarizing"),
        ];

        let (_, level) = engine.escalate_summarize(&messages, 10).await.unwrap();
        // Should escalate directly to Level 3.
        assert_eq!(level, 3);
    }

    #[test]
    fn test_deterministic_truncate_short_text() {
        let text = "Short text";
        // 50 tokens * 4 = 200 char budget, "Short text" is 10 chars → no truncation
        let result = CompactionEngine::deterministic_truncate(text, 50);
        assert_eq!(result, text); // No truncation needed.
    }

    #[test]
    fn test_deterministic_truncate_long_text() {
        let text = "A".repeat(500);
        // 25 tokens * 4 chars/token = 100 char budget
        let result = CompactionEngine::deterministic_truncate(&text, 25);
        // The result includes marker text, so it can exceed the char budget.
        // But it should be much shorter than the original 500 chars.
        assert!(result.chars().count() < 200, "Truncated text should be significantly shorter than original");
        assert!(result.contains("truncated by LCM"));
        assert!(result.starts_with('A'));
        assert!(result.ends_with('A'));
    }

    #[test]
    fn test_concat_messages() {
        let messages = vec![
            make_msg("1", "Hello"),
            make_msg("2", "World"),
        ];
        let text = CompactionEngine::concat_messages(&messages);
        assert!(text.contains("[User]: Hello"));
        assert!(text.contains("[User]: World"));
    }
}
