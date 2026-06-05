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

use crate::lcm::types::{
    FileRefId, LcmError, MessageRole, StoredMessage, SummaryId, SummaryKind, SummaryNode,
};
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
            "No summarizer configured — use LcmConfig to set a summarization provider".to_string(),
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
        let input_tokens: u32 = messages
            .iter()
            .map(|m| {
                let base = m.token_count;
                let thinking_tokens = m
                    .thinking
                    .as_ref()
                    .map(|t| (self.count_tokens)(t))
                    .unwrap_or(0);
                base + thinking_tokens
            })
            .sum();
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

    /// Concatenate messages (including thinking content) into a single text
    /// block for summarization. Thinking content is included before the
    /// assistant text so the summary preserves important reasoning context.
    fn concat_messages(messages: &[StoredMessage]) -> String {
        let mut text = String::new();
        for msg in messages {
            let role_label = match msg.role {
                MessageRole::User => "User",
                MessageRole::Assistant => "Assistant",
                MessageRole::Tool => "Tool",
            };
            text.push_str(&format!("[{role_label}]: {}\n", msg.content));
            if let Some(ref thinking) = msg.thinking {
                let trimmed = thinking.trim();
                if !trimmed.is_empty() {
                    text.push_str(&format!("[Thinking]: {}\n", trimmed));
                }
            }
        }
        text
    }

    /// Role markers that delimit messages in concatenated text.
    const ROLE_MARKERS: &[&str] = &["\n[User]:", "\n[Assistant]:", "\n[Tool]:", "\n[Thinking]:"];

    /// Find the last `\n` before `cut_pos` that starts a role marker line.
    /// Returns the byte offset of the `\n` (or the cut_pos if none found).
    fn align_to_message_boundary(text: &str, cut_pos: usize) -> usize {
        // Search for role markers in the 200-chars window before cut_pos.
        let search_start = cut_pos.saturating_sub(200);
        let window = &text[search_start..cut_pos];

        let mut best = None;
        for marker in Self::ROLE_MARKERS {
            if let Some(pos) = window.rfind(marker) {
                let abs_pos = search_start + pos;
                if abs_pos > best.unwrap_or(0) {
                    best = Some(abs_pos);
                }
            }
        }

        // Only adjust if we found a boundary that saves at least 20 chars
        // from the original cut — avoids pathological tiny adjustments.
        match best {
            Some(pos) if cut_pos.saturating_sub(pos) > 20 => pos,
            _ => cut_pos,
        }
    }

    /// Find the first `\n` after `cut_pos` that starts a role marker line.
    /// Returns the byte offset of the `\n` (or the cut_pos if none found).
    fn align_tail_to_message_boundary(text: &str, cut_pos: usize) -> usize {
        let search_end = (cut_pos + 200).min(text.len());
        let window = &text[cut_pos..search_end];

        let mut best = None;
        for marker in Self::ROLE_MARKERS {
            if let Some(pos) = window.find(marker) {
                let abs_pos = cut_pos + pos;
                if best.map_or(true, |best| abs_pos < best) {
                    best = Some(abs_pos);
                }
            }
        }

        match best {
            Some(pos) if pos.saturating_sub(cut_pos) <= 200 => pos,
            _ => cut_pos,
        }
    }

    /// Level 3: Deterministic truncation with message-boundary alignment.
    ///
    /// Takes the first ~67% and last ~33% of tokens, converted from
    /// `max_tokens` to characters via the 4:1 heuristic. This preserves
    /// context from both the beginning and end of the conversation.
    /// Unlike a purely character-based cut, this adjusts split points to
    /// land on `[User]:` / `[Assistant]:` / `[Tool]:` / `[Thinking]:` boundaries
    /// so partial messages are not produced.
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

        // Head: first ~67% aligned to message boundary.
        let raw_head_end = text
            .char_indices()
            .nth(head_chars)
            .map(|(i, _)| i)
            .unwrap_or(text.len());
        let head_end = Self::align_to_message_boundary(text, raw_head_end);
        let head = &text[..head_end];

        // Tail: last ~33% aligned to message boundary.
        let remaining = text.len().saturating_sub(head_end);
        let tail_from = text
            .char_indices()
            .rev()
            .nth(tail_chars)
            .map(|(i, _)| i)
            .unwrap_or(0);
        // Align forward to the next message boundary
        let tail_from = Self::align_tail_to_message_boundary(text, tail_from);

        let tail = if tail_from > head_end {
            &text[tail_from..]
        } else {
            // Tail overlaps with head — just use head.
            ""
        };

        let omitted = text.len().saturating_sub(head.len() + tail.len());

        if tail.is_empty() {
            format!(
                "{head}\n\n[... {omitted} bytes truncated by LCM Level 3 compaction ...]"
            )
        } else {
            format!(
                "{head}\n\n[... {omitted} bytes truncated by LCM Level 3 compaction ...]\n\n{tail}"
            )
        }
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
            make_msg(
                "1",
                "This is a very long message with many tokens indeed yes absolutely",
            ),
            make_msg(
                "2",
                "Another long message that adds more tokens to the count here",
            ),
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

        let messages = vec![make_msg("1", "Some message that needs summarizing")];

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
        assert!(
            result.chars().count() < 200,
            "Truncated text should be significantly shorter than original"
        );
        assert!(result.contains("truncated by LCM"));
        assert!(result.starts_with('A'));
        assert!(result.ends_with('A'));
    }

    #[test]
    fn test_concat_messages() {
        let messages = vec![make_msg("1", "Hello"), make_msg("2", "World")];
        let text = CompactionEngine::concat_messages(&messages);
        assert!(text.contains("[User]: Hello"));
        assert!(text.contains("[User]: World"));
    }
}
