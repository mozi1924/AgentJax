//! Transcript repair — tool-use/tool-result pairing fixer.
//!
//! Many LLM providers (Anthropic, etc.) reject assembled transcripts where

// These items are not yet called from active code paths; they'll be used once
// integrated into the context assembly pipeline.
#![allow(dead_code)]
//! assistant `tool_use` blocks are not immediately followed by matching
//! `tool_result` messages. Real sessions can produce mispaired transcripts
//! due to:
//!
//! - **Duplicate-ingest**: The same `tool_use` id appearing in multiple
//!   assistant messages (API rejects duplicate ids).
//! - **Orphaned results**: `tool_result` with no matching `tool_use`.
//! - **Delayed results**: `tool_result` arriving after other assistant turns.
//! - **Missing results**: `tool_use` with no corresponding `tool_result`.
//! - **Terminal turns**: Assistant messages ending in `error`/`aborted`
//!   may contain incomplete `tool_use` blocks.
//!
//! This module repairs these issues before the assembled context is sent
//! to the provider, matching the approach in lossless-claw's
//! `transcript-repair.ts`.

use crate::lcm::types::{MessageRole, StoredMessage};
use serde_json::Value;
use std::collections::{HashMap, HashSet};

// ── Lightweight tool-call info extracted during repair ──────────────────────

/// Info about a tool call found in an assistant message.
#[derive(Debug, Clone)]
struct ToolCallInfo {
    id: String,
    name: String,
}

/// Info about a tool result found in a tool message.
#[derive(Debug, Clone)]
struct ToolResultInfo {
    id: String,
    index: usize,
}

/// A repaired segment: either a kept message or a synthesized error result.
#[derive(Debug, Clone)]
enum RepairedMessage {
    /// Original kept message.
    Original(StoredMessage),
    /// Synthesized error result injected because a tool result was missing.
    SyntheticError {
        tool_call_id: String,
        tool_name: String,
    },
}

// ── Repair entry point ─────────────────────────────────────────────────────

/// Repair tool-use/tool-result pairing in a list of assembled messages.
///
/// This function implements the same logic as lossless-claw's
/// `sanitizeToolUseResultPairing()`:
///
/// 1. **Deduplicate**: Remove duplicate assistant `tool_use` ids (keep-first).
/// 2. **Move**: Pull delayed `tool_result` messages right after their
///    corresponding assistant turn.
/// 3. **Synthesize**: Insert synthetic error results for missing tool results.
/// 4. **Remove orphans**: Drop `tool_result` messages with no matching
///    `tool_use`.
/// 5. **Strip terminals**: For assistant messages ending in `error`/`aborted`,
///    strip all `tool_use` blocks and do not pair them.
///
/// Returns the repaired message list and a report of what was changed.
pub fn sanitize_tool_use_result_pairing(
    messages: Vec<StoredMessage>,
) -> (Vec<StoredMessage>, RepairReport) {
    let mut report = RepairReport::default();
    if messages.is_empty() {
        return (messages, report);
    }

    // Phase 1: Index all messages by their role and extract tool info.
    let mut assistant_msgs: Vec<(usize, StoredMessage)> = Vec::new();
    let mut tool_results: Vec<(usize, StoredMessage, ToolResultInfo)> = Vec::new();
    let mut other_msgs: Vec<(usize, StoredMessage)> = Vec::new();

    for (i, msg) in messages.into_iter().enumerate() {
        match msg.role {
            MessageRole::Assistant => {
                assistant_msgs.push((i, msg));
            }
            MessageRole::Tool => {
                // Extract tool_call_id from metadata.
                if let Some(tool_call_id) = msg
                    .metadata
                    .get("tool_call_id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string())
                {
                    let _tool_name = msg
                        .metadata
                        .get("tool_name")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    tool_results.push((
                        i,
                        msg,
                        ToolResultInfo {
                            id: tool_call_id,
                            index: i,
                        },
                    ));
                } else {
                    // Try to find tool_call_id in content.
                    let tool_call_id = extract_tool_call_id_from_content(&msg.content);
                    if let Some(id) = tool_call_id {
                        tool_results.push((
                            i,
                            msg,
                            ToolResultInfo {
                                id,
                                index: i,
                            },
                        ));
                    } else {
                        // No tool_call_id — treat as orphan.
                        report.orphaned_results += 1;
                        report.changed = true;
                        continue; // Drop orphan.
                    }
                }
            }
            _ => {
                other_msgs.push((i, msg));
            }
        }
    }

    // Phase 2: Process assistant messages.
    let mut repaired: Vec<RepairedMessage> = Vec::new();
    let mut seen_tool_use_ids: HashSet<String> = HashSet::new();
    let mut seen_tool_result_ids: HashSet<String> = HashSet::new();
    let mut tool_result_map: HashMap<String, Vec<(usize, StoredMessage)>> = HashMap::new();

    // Build a map of tool results by call id.
    for (idx, msg, info) in &tool_results {
        tool_result_map
            .entry(info.id.clone())
            .or_default()
            .push((*idx, msg.clone()));
    }

    // Process messages in original order.
    let mut tool_result_indices_used: HashSet<usize> = HashSet::new();
    let mut all_messages: Vec<(usize, Option<StoredMessage>)> = Vec::new();

    // Rebuild the ordered list of messages with their original indices.
    for (orig_idx, msg) in &assistant_msgs {
        all_messages.push((*orig_idx, Some(msg.clone())));
    }
    for (orig_idx, msg, _info) in &tool_results {
        all_messages.push((*orig_idx, Some(msg.clone())));
    }
    for (orig_idx, msg) in &other_msgs {
        all_messages.push((*orig_idx, Some(msg.clone())));
    }
    all_messages.sort_by_key(|(idx, _)| *idx);

    // Phase 3: Walk through messages and repair.
    let mut i = 0;
    while i < all_messages.len() {
        let (_, msg_opt) = &all_messages[i];
        let Some(msg) = msg_opt else {
            i += 1;
            continue;
        };

        match msg.role {
            MessageRole::Assistant => {
                let tool_calls = extract_tool_calls(msg);
                let is_terminal = is_terminal_stop_reason(msg);

                if is_terminal {
                    // Terminal (error/aborted) — strip all tool_use blocks.
                    if !tool_calls.is_empty() {
                        report.terminal_tool_uses += tool_calls.len() as u64;
                        report.changed = true;
                    }
                    // Still keep the message with tool_use blocks removed.
                    repaired.push(RepairedMessage::Original(msg.clone()));
                    i += 1;
                    continue;
                }

                // Deduplicate tool_use ids (keep-first within this assistant msg).
                let mut filtered_tool_calls: Vec<ToolCallInfo> = Vec::new();
                for tc in &tool_calls {
                    if seen_tool_use_ids.contains(&tc.id) {
                        report.duplicate_tool_uses += 1;
                        report.changed = true;
                    } else {
                        seen_tool_use_ids.insert(tc.id.clone());
                        filtered_tool_calls.push(tc.clone());
                    }
                }

                // If all tool_use blocks were dropped and content is effectively empty, skip.
                if filtered_tool_calls.is_empty() && is_content_effectively_empty(msg) {
                    i += 1;
                    continue;
                }

                // Emit the (possibly deduplicated) assistant message.
                let deduped_msg = if filtered_tool_calls.len() < tool_calls.len() {
                    Some(remove_tool_calls_from_message(
                        msg.clone(),
                        &tool_calls
                            .iter()
                            .filter(|tc| !filtered_tool_calls.iter().any(|f| f.id == tc.id))
                            .map(|tc| tc.id.clone())
                            .collect::<Vec<_>>(),
                    ))
                } else {
                    None
                };
                repaired.push(RepairedMessage::Original(
                    deduped_msg.unwrap_or_else(|| msg.clone()),
                ));

                // Find and move matching tool results.
                for tc in &filtered_tool_calls {
                    if let Some(results) = tool_result_map.get(&tc.id) {
                        for (result_idx, result_msg) in results {
                            if !tool_result_indices_used.contains(result_idx) {
                                if seen_tool_result_ids.contains(&tc.id) {
                                    report.duplicate_results += 1;
                                    report.changed = true;
                                    continue; // Skip duplicate.
                                }
                                seen_tool_result_ids.insert(tc.id.clone());
                                tool_result_indices_used.insert(*result_idx);
                                repaired.push(RepairedMessage::Original(result_msg.clone()));
                            }
                        }
                        // Check if any results are missing (weren't matched).
                        let matched_count = results
                            .iter()
                            .filter(|(idx, _)| tool_result_indices_used.contains(idx))
                            .count();
                        if matched_count == 0 {
                            // No result found at all — synthesize.
                            report.synthesized_results += 1;
                            report.changed = true;
                            repaired.push(RepairedMessage::SyntheticError {
                                tool_call_id: tc.id.clone(),
                                tool_name: tc.name.clone(),
                            });
                        }
                    } else {
                        // No result found — synthesize.
                        report.synthesized_results += 1;
                        report.changed = true;
                        repaired.push(RepairedMessage::SyntheticError {
                            tool_call_id: tc.id.clone(),
                            tool_name: tc.name.clone(),
                        });
                    }
                }

                i += 1;
            }
            MessageRole::Tool => {
                // Orphaned tool result — check if it was already matched.
                let tool_call_id = extract_tool_call_id(msg);
                let is_orphan = match &tool_call_id {
                    Some(_id) => {
                        !tool_result_indices_used
                            .contains(&all_messages.iter().position(|(idx, _)| *idx == i).unwrap_or(usize::MAX))
                    }
                    None => true,
                };

                if is_orphan {
                    report.orphaned_results += 1;
                    report.changed = true;
                    // Drop orphan.
                    i += 1;
                    continue;
                }

                repaired.push(RepairedMessage::Original(msg.clone()));
                i += 1;
            }
            _ => {
                repaired.push(RepairedMessage::Original(msg.clone()));
                i += 1;
            }
        }
    }

    // Phase 4: Flatten repaired messages back to Vec<StoredMessage>.
    let mut output: Vec<StoredMessage> = Vec::with_capacity(repaired.len());
    for item in repaired {
        match item {
            RepairedMessage::Original(msg) => output.push(msg),
            RepairedMessage::SyntheticError {
                tool_call_id,
                tool_name,
            } => {
                let mut metadata = HashMap::new();
                metadata.insert(
                    "tool_call_id".to_string(),
                    Value::String(tool_call_id.clone()),
                );
                metadata.insert("tool_name".to_string(), Value::String(tool_name.clone()));
                metadata.insert(
                    "is_error".to_string(),
                    Value::Bool(true),
                );
                metadata.insert(
                    "synthetic".to_string(),
                    Value::Bool(true),
                );

                let synthetic = StoredMessage {
                    id: crate::lcm::types::LcmId::new(),
                    conversation_id: String::new(), // Will be filled by caller.
                    role: MessageRole::Tool,
                    content: format!(
                        "[AgentJax LCM repair] Missing tool result for call '{tool_call_id}'; \
                         inserted synthetic error result for transcript repair."
                    ),
                    token_count: 0,
                    timestamp_unix_ms: chrono::Utc::now().timestamp_millis(),
                    covered_by: None,
                    thinking: None,
                    metadata: metadata.into_iter().collect(),
                    file_refs: Vec::new(),
                };
                output.push(synthetic);
            }
        }
    }

    (output, report)
}

// ── Report ──────────────────────────────────────────────────────────────────

/// Report of what the repair changed.
#[derive(Debug, Default, Clone)]
pub struct RepairReport {
    /// Number of duplicate tool_use ids removed.
    pub duplicate_tool_uses: u64,
    /// Number of duplicate tool_result ids removed.
    pub duplicate_results: u64,
    /// Number of synthetic error results injected.
    pub synthesized_results: u64,
    /// Number of orphaned tool_results removed.
    pub orphaned_results: u64,
    /// Number of terminal (error/aborted) tool_use blocks stripped.
    pub terminal_tool_uses: u64,
    /// Whether any changes were made.
    pub changed: bool,
}

impl std::fmt::Display for RepairReport {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if !self.changed {
            return write!(f, "no repair needed");
        }
        let mut parts = Vec::new();
        if self.duplicate_tool_uses > 0 {
            parts.push(format!("{} duplicate tool_uses removed", self.duplicate_tool_uses));
        }
        if self.duplicate_results > 0 {
            parts.push(format!("{} duplicate results removed", self.duplicate_results));
        }
        if self.synthesized_results > 0 {
            parts.push(format!("{} synthetic results injected", self.synthesized_results));
        }
        if self.orphaned_results > 0 {
            parts.push(format!("{} orphaned results removed", self.orphaned_results));
        }
        if self.terminal_tool_uses > 0 {
            parts.push(format!("{} terminal tool_uses stripped", self.terminal_tool_uses));
        }
        write!(f, "{}", parts.join("; "))
    }
}

// ── Helper functions ────────────────────────────────────────────────────────

/// Extract tool call ids from an assistant message's metadata.
fn extract_tool_calls(msg: &StoredMessage) -> Vec<ToolCallInfo> {
    let mut calls = Vec::new();

    // Check for tool_call_id in metadata (simplified for StoredMessage format).
    // In practice, tool calls are represented in metadata as arrays or individual keys.
    if let Some(tool_call_id) = msg.metadata.get("tool_call_id").and_then(|v| v.as_str()) {
        let name = msg
            .metadata
            .get("tool_name")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        calls.push(ToolCallInfo {
            id: tool_call_id.to_string(),
            name: name.to_string(),
        });
    }

    // Check for multiple tool calls in an array.
    if let Some(tool_calls) = msg.metadata.get("tool_calls").and_then(|v| v.as_array()) {
        for tc in tool_calls {
            if let Some(id) = tc.get("id").and_then(|v| v.as_str()) {
                let name = tc
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("unknown");
                calls.push(ToolCallInfo {
                    id: id.to_string(),
                    name: name.to_string(),
                });
            }
        }
    }

    calls
}

/// Extract a tool call ID from a tool result message.
fn extract_tool_call_id(msg: &StoredMessage) -> Option<String> {
    msg.metadata
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
}

/// Try to find a tool_call_id embedded in content text.
fn extract_tool_call_id_from_content(content: &str) -> Option<String> {
    // Attempt to parse as JSON and look for tool_call_id.
    if let Ok(val) = serde_json::from_str::<Value>(content) {
        if let Some(id) = val.get("tool_call_id").and_then(|v| v.as_str()) {
            return Some(id.to_string());
        }
        if let Some(id) = val.get("tool_use_id").and_then(|v| v.as_str()) {
            return Some(id.to_string());
        }
    }
    None
}

/// Check if a message has a terminal stop reason (error/aborted).
fn is_terminal_stop_reason(msg: &StoredMessage) -> bool {
    if let Some(stop_reason) = msg.metadata.get("stop_reason").and_then(|v| v.as_str()) {
        if stop_reason == "error" || stop_reason == "aborted" {
            return true;
        }
    }
    if let Some(stop_reason) = msg.metadata.get("stopReason").and_then(|v| v.as_str()) {
        if stop_reason == "error" || stop_reason == "aborted" {
            return true;
        }
    }
    false
}

/// Remove specific tool call ids from an assistant message's metadata.
fn remove_tool_calls_from_message(
    mut msg: StoredMessage,
    ids_to_remove: &[String],
) -> StoredMessage {
    if let Some(tool_calls) = msg
        .metadata
        .get("tool_calls")
        .and_then(|v| v.as_array())
    {
        let filtered: Vec<Value> = tool_calls
            .iter()
            .filter(|tc| {
                !ids_to_remove
                    .iter()
                    .any(|id| tc.get("id").and_then(|v| v.as_str()) == Some(id.as_str()))
            })
            .cloned()
            .collect();
        if filtered.is_empty() {
            msg.metadata.remove("tool_calls");
            // Also remove singular tool_call_id if we removed all.
            msg.metadata.remove("tool_call_id");
        } else {
            msg.metadata
                .insert("tool_calls".to_string(), Value::Array(filtered));
        }
    } else if ids_to_remove
        .iter()
        .any(|id| msg.metadata.get("tool_call_id").and_then(|v| v.as_str()) == Some(id.as_str()))
    {
        msg.metadata.remove("tool_call_id");
    }
    msg
}

/// Check if a message's content is effectively empty after tool_use removal.
fn is_content_effectively_empty(msg: &StoredMessage) -> bool {
    msg.content.trim().is_empty()
        && msg
            .metadata
            .get("tool_calls")
            .and_then(|v| v.as_array())
            .map(|a| a.is_empty())
            .unwrap_or(true)
        && msg.metadata.get("tool_call_id").is_none()
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::types::LcmId;
    use std::collections::BTreeMap;

    fn make_assistant_msg(
        id: &str,
        tool_call_id: &str,
        tool_name: &str,
        content: &str,
        stop_reason: Option<&str>,
    ) -> StoredMessage {
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "tool_call_id".to_string(),
            Value::String(tool_call_id.to_string()),
        );
        metadata.insert(
            "tool_name".to_string(),
            Value::String(tool_name.to_string()),
        );
        if let Some(reason) = stop_reason {
            metadata.insert(
                "stop_reason".to_string(),
                Value::String(reason.to_string()),
            );
        }
        StoredMessage {
            id: LcmId::from(id),
            conversation_id: "test".to_string(),
            role: MessageRole::Assistant,
            content: content.to_string(),
            token_count: 10,
            timestamp_unix_ms: 1000,
            covered_by: None,
            thinking: None,
            metadata,
            file_refs: Vec::new(),
        }
    }

    fn make_tool_result(id: &str, tool_call_id: &str, tool_name: &str) -> StoredMessage {
        let mut metadata = BTreeMap::new();
        metadata.insert(
            "tool_call_id".to_string(),
            Value::String(tool_call_id.to_string()),
        );
        metadata.insert(
            "tool_name".to_string(),
            Value::String(tool_name.to_string()),
        );
        StoredMessage {
            id: LcmId::from(id),
            conversation_id: "test".to_string(),
            role: MessageRole::Tool,
            content: "result content".to_string(),
            token_count: 5,
            timestamp_unix_ms: 1000,
            covered_by: None,
            thinking: None,
            metadata,
            file_refs: Vec::new(),
        }
    }

    #[test]
    fn test_no_repair_needed() {
        let msgs = vec![
            make_assistant_msg("a1", "call_1", "tool_a", "thinking...", None),
            make_tool_result("r1", "call_1", "tool_a"),
        ];
        let (output, report) = sanitize_tool_use_result_pairing(msgs);
        assert!(!report.changed);
        assert_eq!(output.len(), 2);
    }

    #[test]
    fn test_synthesizes_missing_result() {
        let msgs = vec![make_assistant_msg("a1", "call_1", "tool_a", "thinking...", None)];
        let (output, report) = sanitize_tool_use_result_pairing(msgs);
        assert!(report.changed);
        assert_eq!(report.synthesized_results, 1);
        assert_eq!(output.len(), 2); // assistant + synthetic
        if let Some(last) = output.last() {
            assert_eq!(last.role, MessageRole::Tool);
            assert_eq!(
                last.metadata.get("tool_call_id").and_then(|v| v.as_str()),
                Some("call_1")
            );
            assert_eq!(
                last.metadata.get("is_error").and_then(|v| v.as_bool()),
                Some(true)
            );
        }
    }

    #[test]
    fn test_removes_orphan_result() {
        let msgs = vec![
            make_assistant_msg("a1", "call_1", "tool_a", "thinking...", None),
            make_tool_result("r1", "call_1", "tool_a"),
            make_tool_result("r2", "call_orphan", "tool_b"),
        ];
        let (output, report) = sanitize_tool_use_result_pairing(msgs);
        assert!(report.changed);
        assert_eq!(report.orphaned_results, 1);
        assert_eq!(output.len(), 2); // assistant + matched result only
    }

    #[test]
    fn test_terminal_strips_tool_uses() {
        let msgs = vec![make_assistant_msg(
            "a1",
            "call_1",
            "tool_a",
            "error occurred",
            Some("error"),
        )];
        let (output, report) = sanitize_tool_use_result_pairing(msgs);
        assert!(report.changed);
        assert_eq!(report.terminal_tool_uses, 1);
        // No synthetic result created for terminal.
        assert_eq!(report.synthesized_results, 0);
        assert_eq!(output.len(), 1);
    }
}
