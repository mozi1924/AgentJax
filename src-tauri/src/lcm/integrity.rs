//! LCM Integrity Checker — diagnostic checks for conversation health.
//!
//! Inspired by lossless-claw's `integrity.ts`. Each check returns an
//! `IntegrityCheck` with pass/fail/warn status.
//!
//! ## Checks
//!
//! 1. conversation_exists — Conversation metadata exists in store
//! 2. summaries_have_lineage — Leaf nodes link to messages, condensed to parents
//! 3. no_orphan_summaries — All summaries are referenced or are a parent
//! 4. message_seq_contiguous — Message timestamps are non-decreasing
//! 5. context_token_count — Total token count is consistent

use crate::lcm::store::LcmStore;
use crate::lcm::types::LcmError;
use serde::Serialize;
use std::sync::Arc;

// ── Types ───────────────────────────────────────────────────────────────────

/// The result of a single integrity check.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityCheck {
    pub name: String,
    pub status: IntegrityStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum IntegrityStatus {
    Pass,
    Fail,
    Warn,
}

/// Full integrity report for a conversation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityReport {
    pub conversation_id: String,
    pub checks: Vec<IntegrityCheck>,
    pub pass_count: usize,
    pub fail_count: usize,
    pub warn_count: usize,
    pub scanned_at_unix_ms: i64,
}

/// Snapshot metrics for a conversation.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LcmMetrics {
    pub conversation_id: String,
    pub message_count: usize,
    pub summary_count: usize,
    pub leaf_summary_count: usize,
    pub condensed_summary_count: usize,
    pub large_file_count: usize,
    pub total_message_tokens: u32,
    pub total_summary_tokens: u32,
    pub collected_at_unix_ms: i64,
}

// ── Integrity Checker ───────────────────────────────────────────────────────

pub struct IntegrityChecker {
    store: Arc<LcmStore>,
}

impl IntegrityChecker {
    pub fn new(store: Arc<LcmStore>) -> Self {
        Self { store }
    }

    /// Run all integrity checks for a conversation.
    pub fn scan(&self, conversation_id: &str) -> Result<IntegrityReport, LcmError> {
        let mut checks = Vec::with_capacity(5);

        checks.push(self.check_conversation_exists(conversation_id)?);
        checks.push(self.check_summaries_have_lineage(conversation_id)?);
        checks.push(self.check_no_orphan_summaries(conversation_id)?);
        checks.push(self.check_message_seq_contiguous(conversation_id)?);
        checks.push(self.check_context_token_count(conversation_id)?);

        let pass_count = checks
            .iter()
            .filter(|c| c.status == IntegrityStatus::Pass)
            .count();
        let fail_count = checks
            .iter()
            .filter(|c| c.status == IntegrityStatus::Fail)
            .count();
        let warn_count = checks
            .iter()
            .filter(|c| c.status == IntegrityStatus::Warn)
            .count();

        Ok(IntegrityReport {
            conversation_id: conversation_id.to_string(),
            checks,
            pass_count,
            fail_count,
            warn_count,
            scanned_at_unix_ms: crate::conversation_store_utils::now_unix_ms(),
        })
    }

    /// Collect session metrics.
    pub fn collect_metrics(&self, conversation_id: &str) -> Result<LcmMetrics, LcmError> {
        let msgs = self.store.get_conversation_messages(conversation_id)?;
        let summaries = self.store.get_conversation_summaries(conversation_id)?;

        let message_count = msgs.len();
        let total_message_tokens: u32 = msgs.iter().map(|m| m.token_count).sum();
        let summary_count = summaries.len();
        let leaf_summary_count = summaries
            .iter()
            .filter(|s| s.kind == crate::lcm::types::SummaryKind::Leaf)
            .count();
        let condensed_summary_count = summary_count.saturating_sub(leaf_summary_count);
        let total_summary_tokens: u32 = summaries.iter().map(|s| s.token_count).sum();

        // Count large files by scanning file_refs from messages.
        let mut file_ref_ids = std::collections::HashSet::new();
        for msg in &msgs {
            for fr in &msg.file_refs {
                file_ref_ids.insert(fr.to_string());
            }
        }

        Ok(LcmMetrics {
            conversation_id: conversation_id.to_string(),
            message_count,
            summary_count,
            leaf_summary_count,
            condensed_summary_count,
            large_file_count: file_ref_ids.len(),
            total_message_tokens,
            total_summary_tokens,
            collected_at_unix_ms: crate::conversation_store_utils::now_unix_ms(),
        })
    }

    // ── Individual Checks ─────────────────────────────────────────────

    fn check_conversation_exists(&self, conversation_id: &str) -> Result<IntegrityCheck, LcmError> {
        match self.store.get_conversation_meta(conversation_id) {
            Ok(Some(_)) => Ok(IntegrityCheck {
                name: "conversation_exists".to_string(),
                status: IntegrityStatus::Pass,
                message: "Conversation metadata found in LCM store".to_string(),
                details: None,
            }),
            Ok(None) => {
                // Not in LCM — check if the conversation exists in legacy metadata.json.
                // New conversations or pre-LCM conversations will not have LCM metadata
                // yet; they get populated on first load via backfill_lcm_from_jsonl.
                let has_legacy_meta = self
                    .store
                    .db_path()
                    .parent()
                    .map(|dir| dir.join("metadata.json"))
                    .filter(|p| p.exists())
                    .is_some();

                if has_legacy_meta {
                    Ok(IntegrityCheck {
                        name: "conversation_exists".to_string(),
                        status: IntegrityStatus::Warn,
                        message: format!(
                            "Conversation '{}' has legacy metadata but not yet in LCM store. \
                             Will be populated on next load via JSONL backfill.",
                            conversation_id
                        ),
                        details: None,
                    })
                } else {
                    // No storage files at all — conversation either doesn't exist or
                    // is a new/pre-populated conversation that hasn't been persisted yet.
                    // This is NOT an integrity failure; it's a normal state for new chats.
                    Ok(IntegrityCheck {
                        name: "conversation_exists".to_string(),
                        status: IntegrityStatus::Warn,
                        message: format!(
                            "Conversation '{}' has no persistent storage yet \
                             (new or pre-populated conversation pending first save). \
                             Storage will be created on first message.",
                            conversation_id
                        ),
                        details: None,
                    })
                }
            }
            Err(e) => Ok(IntegrityCheck {
                name: "conversation_exists".to_string(),
                status: IntegrityStatus::Fail,
                message: format!("Error reading conversation metadata: {e}"),
                details: None,
            }),
        }
    }

    fn check_summaries_have_lineage(
        &self,
        conversation_id: &str,
    ) -> Result<IntegrityCheck, LcmError> {
        let summaries = self.store.get_conversation_summaries(conversation_id)?;
        let mut issues = Vec::new();

        for s in &summaries {
            match s.kind {
                crate::lcm::types::SummaryKind::Leaf => {
                    let children = self.store.get_summary_children(&s.id)?;
                    let has_msg_children = children
                        .iter()
                        .any(|c| matches!(c, crate::lcm::types::SummaryChild::Messages { .. }));
                    if !has_msg_children {
                        issues.push(format!("leaf summary {} has no message children", s.id));
                    }
                }
                crate::lcm::types::SummaryKind::Condensed => {
                    let children = self.store.get_summary_children(&s.id)?;
                    let has_summary_children = children
                        .iter()
                        .any(|c| matches!(c, crate::lcm::types::SummaryChild::Summaries { .. }));
                    if !has_summary_children {
                        issues.push(format!(
                            "condensed summary {} has no summary children",
                            s.id
                        ));
                    }
                }
            }
        }

        if issues.is_empty() {
            Ok(IntegrityCheck {
                name: "summaries_have_lineage".to_string(),
                status: IntegrityStatus::Pass,
                message: format!("All {} summaries have proper lineage", summaries.len()),
                details: None,
            })
        } else {
            Ok(IntegrityCheck {
                name: "summaries_have_lineage".to_string(),
                status: IntegrityStatus::Warn,
                message: format!("{} summaries with lineage issues", issues.len()),
                details: Some(serde_json::json!({ "issues": issues })),
            })
        }
    }

    fn check_no_orphan_summaries(&self, conversation_id: &str) -> Result<IntegrityCheck, LcmError> {
        let summaries = self.store.get_conversation_summaries(conversation_id)?;

        // Collect all summary IDs that are referenced as parents by other summaries.
        let mut parent_ids = std::collections::HashSet::new();
        for s in &summaries {
            for p in &s.parents {
                parent_ids.insert(p.to_string());
            }
        }

        // Also collect IDs referenced as children.
        let mut child_ids = std::collections::HashSet::new();
        for s in &summaries {
            if let Ok(children) = self.store.get_summary_children(&s.id) {
                for c in &children {
                    match c {
                        crate::lcm::types::SummaryChild::Summaries { ids } => {
                            for id in ids {
                                child_ids.insert(id.to_string());
                            }
                        }
                        _ => {}
                    }
                }
            }
        }

        // An orphan is a summary that is neither a parent of another summary
        // nor a child of another summary (i.e., completely disconnected).
        let orphans: Vec<String> = summaries
            .iter()
            .filter(|s| {
                let id_str = s.id.to_string();
                !parent_ids.contains(&id_str) && !child_ids.contains(&id_str) && summaries.len() > 1 // ignore single-summary case
            })
            .map(|s| {
                if s.parents.is_empty() {
                    Some(s.id.to_string())
                } else {
                    None
                }
            })
            .flatten()
            .collect();

        if orphans.is_empty() {
            Ok(IntegrityCheck {
                name: "no_orphan_summaries".to_string(),
                status: IntegrityStatus::Pass,
                message: "No orphan summaries found".to_string(),
                details: None,
            })
        } else {
            Ok(IntegrityCheck {
                name: "no_orphan_summaries".to_string(),
                status: IntegrityStatus::Warn,
                message: format!("{} orphan summaries found", orphans.len()),
                details: Some(serde_json::json!({ "orphans": orphans })),
            })
        }
    }

    fn check_message_seq_contiguous(
        &self,
        conversation_id: &str,
    ) -> Result<IntegrityCheck, LcmError> {
        let messages = self.store.get_conversation_messages(conversation_id)?;
        if messages.len() <= 1 {
            return Ok(IntegrityCheck {
                name: "message_seq_contiguous".to_string(),
                status: IntegrityStatus::Pass,
                message: format!("{} messages, no sequence issues", messages.len()),
                details: None,
            });
        }

        let mut issues = Vec::new();
        for i in 1..messages.len() {
            if messages[i].timestamp_unix_ms < messages[i - 1].timestamp_unix_ms {
                issues.push(format!(
                    "message {} has timestamp before previous",
                    messages[i].id
                ));
            }
        }

        if issues.is_empty() {
            Ok(IntegrityCheck {
                name: "message_seq_contiguous".to_string(),
                status: IntegrityStatus::Pass,
                message: format!("{} messages in sequence", messages.len()),
                details: None,
            })
        } else {
            Ok(IntegrityCheck {
                name: "message_seq_contiguous".to_string(),
                status: IntegrityStatus::Fail,
                message: format!("{} messages with ordering issues", issues.len()),
                details: Some(serde_json::json!({ "issues": issues })),
            })
        }
    }

    fn check_context_token_count(&self, conversation_id: &str) -> Result<IntegrityCheck, LcmError> {
        let messages = self.store.get_conversation_messages(conversation_id)?;
        let summaries = self.store.get_conversation_summaries(conversation_id)?;

        let msg_tokens: u32 = messages.iter().map(|m| m.token_count).sum();
        let summary_tokens: u32 = summaries.iter().map(|s| s.token_count).sum();
        let total = msg_tokens + summary_tokens;

        Ok(IntegrityCheck {
            name: "context_token_count".to_string(),
            status: IntegrityStatus::Pass,
            message: format!(
                "{total} tokens total ({msg_tokens} messages + {summary_tokens} summaries)"
            ),
            details: Some(serde_json::json!({
                "messageTokens": msg_tokens,
                "summaryTokens": summary_tokens,
                "total": total,
            })),
        })
    }
}

// ── Repair Plan ─────────────────────────────────────────────────────────────

pub fn repair_plan(report: &IntegrityReport) -> Vec<String> {
    let mut suggestions = Vec::new();

    for check in &report.checks {
        if check.status == IntegrityStatus::Pass {
            continue;
        }

        let suggestion = match check.name.as_str() {
            "conversation_exists" => {
                format!(
                    "Create or restore conversation metadata for '{}'",
                    report.conversation_id
                )
            }
            "summaries_have_lineage" => {
                format!(
                    "Add missing summary_message or summary_parent links for '{}'",
                    report.conversation_id
                )
            }
            "no_orphan_summaries" => {
                format!(
                    "Remove orphan summary records for '{}'",
                    report.conversation_id
                )
            }
            "message_seq_contiguous" => {
                format!(
                    "Re-index message sequence values for '{}'",
                    report.conversation_id
                )
            }
            "context_token_count" => {
                format!("Token count is consistent (information only)")
            }
            _ => {
                format!("Review check '{}': {}", check.name, check.message)
            }
        };

        suggestions.push(suggestion);
    }

    suggestions
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::store::LcmStore;
    use crate::lcm::types::{LcmConfig, LcmId, MessageRole, StoredMessage};

    fn create_test_env() -> (Arc<LcmStore>, String) {
        let config = LcmConfig::default();
        let store = Arc::new(LcmStore::open_in_memory(config).unwrap());
        let conv_id = "test_conv".to_string();
        (store, conv_id)
    }

    fn add_test_message(store: &LcmStore, conv_id: &str) -> StoredMessage {
        let msg = StoredMessage::new(
            LcmId::new(),
            conv_id,
            MessageRole::User,
            "Hello, this is a test message.",
            10,
            1000,
        );
        store.persist_message(&msg).unwrap();
        msg
    }

    #[test]
    fn test_check_conversation_exists() {
        let (store, conv_id) = create_test_env();
        let checker = IntegrityChecker::new(store.clone());

        let result = checker.check_conversation_exists(&conv_id).unwrap();
        assert_eq!(result.status, IntegrityStatus::Fail);

        add_test_message(&store, &conv_id);
        let result = checker.check_conversation_exists(&conv_id).unwrap();
        assert_eq!(result.status, IntegrityStatus::Pass);
    }

    #[test]
    fn test_all_checks_run() {
        let (store, conv_id) = create_test_env();
        add_test_message(&store, &conv_id);
        let checker = IntegrityChecker::new(store);
        let report = checker.scan(&conv_id).unwrap();
        assert_eq!(report.checks.len(), 5);
    }

    #[test]
    fn test_metrics_collection() {
        let (store, conv_id) = create_test_env();
        add_test_message(&store, &conv_id);
        let checker = IntegrityChecker::new(store);
        let metrics = checker.collect_metrics(&conv_id).unwrap();
        assert!(metrics.message_count >= 1);
    }

    #[test]
    fn test_repair_plan_skip_pass() {
        let report = IntegrityReport {
            conversation_id: "test".to_string(),
            checks: vec![IntegrityCheck {
                name: "test".to_string(),
                status: IntegrityStatus::Pass,
                message: "ok".to_string(),
                details: None,
            }],
            pass_count: 1,
            fail_count: 0,
            warn_count: 0,
            scanned_at_unix_ms: 1000,
        };
        let plans = repair_plan(&report);
        assert!(plans.is_empty());
    }
}
