//! Focus Briefs — summary-driven context briefs for long sessions.
//!
//! A focus brief is a concise summary of the current conversation state,
//! generated from the summary DAG. It helps the model maintain awareness
//! of the full session context, especially after multiple rounds of
//! compaction.
//!
//! ## Two-phase workflow
//!
//! 1. **Evidence gathering**: A sub-agent examines summaries via `lcm_describe`
//!    and `lcm_expand` to collect relevant context.
//! 2. **Synthesis**: The evidence is condensed into a brief markdown document.
//!
//! The brief includes: narrative summary, key decisions, open questions,
//! file references, and hints for which summaries to expand for more detail.
//!
//! Inspired by lossless-claw's `focus-briefs.ts`.

use crate::lcm::store::LcmStore;
use crate::lcm::types::{LcmError, SummaryKind, SummaryNode};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

// ── Types ───────────────────────────────────────────────────────────────────

/// Generation status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FocusBriefStatus {
    /// Brief is current and accurate.
    Current,
    /// Brief is stale — the summary DAG has changed since generation.
    Stale,
    /// Brief is being generated.
    Generating,
}

/// A single expansion prompt — a question the model may need answered
/// by expanding a specific set of summaries.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExpansionPrompt {
    /// The question or topic to investigate.
    pub prompt: String,
    /// Summary IDs that are relevant to this prompt.
    pub summary_ids: Vec<String>,
}

/// A generated focus brief.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FocusBrief {
    /// Unique identifier for this brief.
    pub id: String,
    /// The conversation this brief belongs to.
    pub conversation_id: String,
    /// The brief markdown text.
    pub markdown: String,
    /// Status of this brief.
    pub status: FocusBriefStatus,
    /// Estimated token count of the brief.
    pub token_count: u32,
    /// When this brief was generated (unix ms).
    pub generated_at_unix_ms: i64,
    /// Summary IDs that were cited in this brief.
    pub cited_summary_ids: Vec<String>,
    /// Summary IDs that were expanded during evidence gathering.
    pub expanded_summary_ids: Vec<String>,
    /// Questions for potential follow-up expansion.
    pub expansion_prompts: Vec<ExpansionPrompt>,
    /// Notes about confidence/coverage.
    pub confidence_notes: Vec<String>,
    /// Whether the brief was truncated due to token limits.
    pub truncated: bool,
}

/// Configuration for focus brief generation.
#[derive(Debug, Clone)]
pub struct FocusBriefConfig {
    /// Target token count for the brief.
    pub target_tokens: u32,
    /// Maximum tokens for expansion during evidence gathering.
    pub max_expand_tokens: u32,
    /// Sub-agent timeout in milliseconds.
    pub delegation_timeout_ms: u64,
}

impl Default for FocusBriefConfig {
    fn default() -> Self {
        Self {
            target_tokens: 2048,
            max_expand_tokens: 4096,
            delegation_timeout_ms: 60_000,
        }
    }
}

// ── Focus Brief Generator ──────────────────────────────────────────────────

/// Generates focus briefs by examining the summary DAG.
pub struct FocusBriefGenerator {
    store: Arc<LcmStore>,
    config: FocusBriefConfig,
}

impl FocusBriefGenerator {
    /// Create a new focus brief generator.
    pub fn new(store: Arc<LcmStore>, config: FocusBriefConfig) -> Self {
        Self { store, config }
    }

    /// Generate a focus brief from the summary DAG.
    ///
    /// This is a simplified single-pass implementation that:
    /// 1. Collects all summaries from the store
    /// 2. Formats them into a structured brief
    /// 3. Identifies key topics and open questions
    ///
    /// For the full two-pass (evidence + synthesis) workflow, a sub-agent
    /// should be used to perform the evidence gathering and synthesis.
    pub fn generate_brief(&self, conversation_id: &str) -> Result<FocusBrief, LcmError> {
        let summaries = self.store.get_conversation_summaries(conversation_id)?;
        let now_ms = crate::conversation_store_utils::now_unix_ms();

        if summaries.is_empty() {
            return Ok(FocusBrief {
                id: format!("brief_{now_ms}"),
                conversation_id: conversation_id.to_string(),
                markdown: "*No summaries available yet.*".to_string(),
                status: FocusBriefStatus::Current,
                token_count: 8,
                generated_at_unix_ms: now_ms,
                cited_summary_ids: Vec::new(),
                expanded_summary_ids: Vec::new(),
                expansion_prompts: Vec::new(),
                confidence_notes: vec!["No summaries to analyze".to_string()],
                truncated: false,
            });
        }

        // Collect leaf summaries for evidence.
        let leaf_summaries: Vec<&SummaryNode> = summaries
            .iter()
            .filter(|s| s.kind == SummaryKind::Leaf)
            .collect();

        let condensed_summaries: Vec<&SummaryNode> = summaries
            .iter()
            .filter(|s| s.kind == SummaryKind::Condensed)
            .collect();

        // Build the brief markdown.
        let mut sections: Vec<String> = Vec::new();
        let mut cited_ids: Vec<String> = Vec::new();
        let mut expansion_prompts: Vec<ExpansionPrompt> = Vec::new();

        sections.push("# Focus Brief\n".to_string());
        sections.push(format!(
            "Generated at: {}\n\n",
            chrono::DateTime::from_timestamp_millis(now_ms)
                .map(|dt| dt.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                .unwrap_or_else(|| "unknown".to_string())
        ));

        // Overview section.
        sections.push(format!(
            "## Overview\n\n- {} messages summarized\n- {} leaf summaries\n- {} condensed summaries\n- Compaction levels: 1 (normal), 2 (aggressive), 3 (truncation)\n",
            leaf_summaries.iter().map(|s| s.token_count).sum::<u32>(),
            leaf_summaries.len(),
            condensed_summaries.len(),
        ));

        // Leaf summaries section — the core evidence.
        if !leaf_summaries.is_empty() {
            sections.push("## Key Topics\n\n".to_string());
            for (i, summary) in leaf_summaries.iter().enumerate() {
                let level_label = match summary.compaction_level {
                    1 => "normal",
                    2 => "aggressive",
                    3 => "truncation",
                    _ => "unknown",
                };
                sections.push(format!(
                    "### Topic {} (Level {}, {} tokens)\n\n{}\n\n",
                    i + 1,
                    level_label,
                    summary.token_count,
                    summary.text,
                ));
                cited_ids.push(summary.id.to_string());

                // Generate an expansion prompt for each topic.
                expansion_prompts.push(ExpansionPrompt {
                    prompt: format!(
                        "Expand topic {} for more details (use lcm_expand on summary {})",
                        i + 1,
                        summary.id
                    ),
                    summary_ids: vec![summary.id.to_string()],
                });
            }
        }

        // Condensed summaries — higher-level patterns.
        if !condensed_summaries.is_empty() {
            sections.push("## Cross-Cutting Themes\n\n".to_string());
            for (i, summary) in condensed_summaries.iter().enumerate() {
                sections.push(format!(
                    "### Theme {} ({} tokens)\n\n{}\n\n",
                    i + 1,
                    summary.token_count,
                    summary.text,
                ));
                cited_ids.push(summary.id.to_string());
            }
        }

        // File references.
        let mut file_refs_set = std::collections::HashSet::new();
        for s in &summaries {
            for f in &s.file_refs {
                file_refs_set.insert(f.to_string());
            }
        }
        if !file_refs_set.is_empty() {
            sections.push(format!(
                "## Files Referenced\n\n- {}\n",
                file_refs_set
                    .iter()
                    .cloned()
                    .collect::<Vec<_>>()
                    .join("\n- ")
            ));
        }

        // Expansion hints.
        if !expansion_prompts.is_empty() {
            sections.push("## Suggested Expansions\n\n".to_string());
            for ep in &expansion_prompts {
                sections.push(format!("- {}\n", ep.prompt));
            }
        }

        let mut markdown = sections.concat();

        // Truncate if too long.
        let target_chars = (self.config.target_tokens as usize).saturating_mul(4);
        let truncated = markdown.len() > target_chars;
        if truncated {
            let truncate_at = mark_dn_truncation_point(&markdown, target_chars);
            markdown.truncate(truncate_at);
            markdown.push_str("\n\n*[Brief truncated to fit token budget]*\n");
        }

        let token_count = crate::lcm::types::estimate_tokens(&markdown);

        Ok(FocusBrief {
            id: format!("brief_{now_ms}"),
            conversation_id: conversation_id.to_string(),
            markdown,
            status: FocusBriefStatus::Current,
            token_count,
            generated_at_unix_ms: now_ms,
            cited_summary_ids: cited_ids,
            expanded_summary_ids: Vec::new(),
            expansion_prompts,
            confidence_notes: vec![],
            truncated,
        })
    }

    /// Check if a brief is stale by comparing with current summary state.
    pub fn is_brief_stale(
        &self,
        brief: &FocusBrief,
        conversation_id: &str,
    ) -> Result<bool, LcmError> {
        let current = self.store.get_conversation_summaries(conversation_id)?;
        let current_ids: std::collections::HashSet<String> =
            current.iter().map(|s| s.id.to_string()).collect();

        // If any cited summary no longer exists, the brief is stale.
        for cited_id in &brief.cited_summary_ids {
            if !current_ids.contains(cited_id) {
                return Ok(true);
            }
        }

        // If there are new summaries that weren't in the brief, it's stale.
        let brief_ids: std::collections::HashSet<String> =
            brief.cited_summary_ids.iter().cloned().collect();
        for current_id in &current_ids {
            if !brief_ids.contains(current_id) {
                return Ok(true);
            }
        }

        Ok(false)
    }
}

/// Find a good truncation point at or before `max_chars`.
fn mark_dn_truncation_point(text: &str, max_chars: usize) -> usize {
    if text.len() <= max_chars {
        return text.len();
    }
    // Try to break at a section boundary (##).
    let truncated = &text[..max_chars];
    if let Some(pos) = truncated.rfind("\n## ") {
        return pos;
    }
    // Try to break at a sentence boundary.
    if let Some(pos) = truncated.rfind(". ") {
        return pos + 1;
    }
    // Just break at the character limit.
    max_chars
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::store::LcmStore;
    use crate::lcm::types::{LcmConfig, SummaryId, SummaryKind, SummaryNode};

    fn create_test_store() -> (Arc<LcmStore>, String) {
        let config = LcmConfig::default();
        let store = Arc::new(LcmStore::open_in_memory(config).unwrap());
        let conv_id = "test_conv".to_string();
        (store, conv_id)
    }

    fn add_leaf_summary(store: &LcmStore, conv_id: &str, text: &str, level: u8) -> SummaryNode {
        let node = SummaryNode {
            id: SummaryId::new(),
            conversation_id: conv_id.to_string(),
            kind: SummaryKind::Leaf,
            text: text.to_string(),
            token_count: crate::lcm::types::estimate_tokens(text),
            created_at_unix_ms: 1000,
            compaction_level: level,
            parents: Vec::new(),
            file_refs: Vec::new(),
        };
        store.insert_summary(&node).unwrap();
        node
    }

    #[test]
    fn test_generate_brief_empty() {
        let (store, conv_id) = create_test_store();
        let generator = FocusBriefGenerator::new(store, FocusBriefConfig::default());
        let brief = generator.generate_brief(&conv_id).unwrap();
        assert!(brief.markdown.contains("No summaries"));
        assert_eq!(brief.status, FocusBriefStatus::Current);
    }

    #[test]
    fn test_generate_brief_with_summaries() {
        let (store, conv_id) = create_test_store();
        add_leaf_summary(
            &store,
            &conv_id,
            "User asked about Rust generics. Assistant explained trait bounds.",
            1,
        );
        add_leaf_summary(
            &store,
            &conv_id,
            "Discussed error handling with Result and Option types.",
            1,
        );
        let generator = FocusBriefGenerator::new(store, FocusBriefConfig::default());
        let brief = generator.generate_brief(&conv_id).unwrap();
        assert!(brief.markdown.contains("Rust generics"));
        assert!(brief.markdown.contains("Key Topics"));
        assert!(brief.markdown.contains("Suggested Expansions"));
        assert_eq!(brief.cited_summary_ids.len(), 2);
    }

    #[test]
    fn test_is_brief_stale() {
        let (store, conv_id) = create_test_store();
        let generator = FocusBriefGenerator::new(store.clone(), FocusBriefConfig::default());

        // Generate brief.
        let brief = generator.generate_brief(&conv_id).unwrap();
        assert!(!generator.is_brief_stale(&brief, &conv_id).unwrap());

        // Add a new summary — should become stale.
        add_leaf_summary(&store, &conv_id, "New topic.", 1);
        assert!(generator.is_brief_stale(&brief, &conv_id).unwrap());
    }
}
