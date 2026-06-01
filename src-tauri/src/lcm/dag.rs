//! Summary DAG (Directed Acyclic Graph) management.
//!
//! The DAG is the core data structure that enables lossless context
//! compression with hierarchical summaries. Each node is a `SummaryNode`,
//! and edges represent "summarizes/condenses" relationships.
//!
//! ## Structure
//!
//! ```text
//!                     [Condensed Summary L3]
//!                    /                      \
//!           [Leaf Summary L2-A]      [Leaf Summary L2-B]
//!          /        |        \        /        |       \
//!     [msg1]    [msg2]    [msg3]  [msg4]   [msg5]   [msg6]
//! ```
//!
//! - **Leaf summaries** (L2-A, L2-B) directly summarize raw messages
//! - **Condensed summaries** (L3) summarize multiple leaf summaries
//! - The original messages are always retained in the Immutable Store

use crate::lcm::store::LcmStore;
use crate::lcm::types::{
    LcmError, LcmId, MessageId, SummaryChild, SummaryId, SummaryKind, SummaryNode,
};
#[cfg(test)]
use crate::lcm::types::StoredMessage;
use std::collections::{HashSet, VecDeque};
use std::sync::Arc;

/// Manages the summary DAG.
///
/// Provides operations for creating, linking, and traversing summary nodes.
/// All persistent state is stored in the LcmStore; this struct provides
/// the in-memory view and algorithms.
#[derive(Clone)]
pub struct SummaryDag {
    store: Arc<LcmStore>,
}

impl SummaryDag {
    /// Create a new DAG manager backed by the given store.
    pub fn new(store: Arc<LcmStore>) -> Self {
        Self { store }
    }

    // ── Node Creation ──────────────────────────────────────────────────

    /// Create a new leaf summary node that compresses a set of raw messages.
    ///
    /// This performs the database operations to:
    /// 1. Insert the summary node
    /// 2. Add child edges pointing to the messages
    /// 3. Mark the messages as covered by this summary
    pub fn create_leaf_summary(
        &self,
        summary: &SummaryNode,
        message_ids: &[MessageId],
    ) -> Result<(), LcmError> {
        if summary.kind != SummaryKind::Leaf {
            return Err(LcmError::Dag(
                "Leaf summary must have kind 'Leaf'".to_string(),
            ));
        }

        // Insert the summary node.
        self.store.insert_summary(summary)?;

        // Add child edges.
        let child = SummaryChild::Messages {
            ids: message_ids.to_vec(),
        };
        self.store.add_summary_child(&summary.id, &child)?;

        // Mark messages as covered.
        self.store.mark_messages_covered(message_ids, &summary.id)?;

        Ok(())
    }

    /// Create a new condensed summary node that compresses multiple
    /// existing summaries.
    pub fn create_condensed_summary(
        &self,
        summary: &SummaryNode,
        child_summary_ids: &[SummaryId],
    ) -> Result<(), LcmError> {
        if summary.kind != SummaryKind::Condensed {
            return Err(LcmError::Dag(
                "Condensed summary must have kind 'Condensed'".to_string(),
            ));
        }

        if child_summary_ids.is_empty() {
            return Err(LcmError::Dag(
                "Condensed summary must have at least one child summary".to_string(),
            ));
        }

        // Insert the summary node.
        self.store.insert_summary(summary)?;

        // Add child edges pointing to the child summaries.
        let child = SummaryChild::Summaries {
            ids: child_summary_ids.to_vec(),
        };
        self.store.add_summary_child(&summary.id, &child)?;

        // Write parent back-references to each child summary.
        for child_id in child_summary_ids {
            self.store.add_summary_parent(child_id, &summary.id)?;
        }

        Ok(())
    }

    // ── Traversal ─────────────────────────────────────────────────────

    /// Get all ancestor summaries for a given node (messages or summaries).
    ///
    /// Traverses upward through the DAG from `covered_by` references.
    /// Returns summaries ordered from nearest ancestor to root.
    pub fn get_ancestors(&self, start_id: &LcmId) -> Result<Vec<SummaryNode>, LcmError> {
        let mut ancestors = Vec::new();
        let mut visited = HashSet::new();

        // First, find the immediate parent (from covered_by).
        let mut current_id: Option<SummaryId> = None;

        // Check if start_id is a message.
        if let Some(msg) = self.store.get_message(&MessageId::from(start_id.as_str()))? {
            current_id = msg.covered_by;
        }

        // Walk up the DAG.
        while let Some(sid) = current_id {
            if !visited.insert(sid.to_string()) {
                // Cycle detected — should not happen in a valid DAG.
                break;
            }

            if let Some(node) = self.store.get_summary(&sid)? {
                current_id = node.parents.first().cloned();
                ancestors.push(node);
            } else {
                break;
            }
        }

        // Reverse so nearest ancestor is first.
        // Actually, we walked from child to parent, so the order is
        // already nearest-to-root. Let's just keep it as-is.

        Ok(ancestors)
    }

    /// Get all descendant message IDs for a summary node.
    ///
    /// Performs a BFS/DFS through the DAG to collect all leaf messages.
    pub fn get_descendant_messages(
        &self,
        summary_id: &SummaryId,
    ) -> Result<Vec<MessageId>, LcmError> {
        let mut messages = Vec::new();
        let mut visited = HashSet::new();
        let mut queue = VecDeque::new();

        queue.push_back(summary_id.clone());

        while let Some(current_id) = queue.pop_front() {
            if !visited.insert(current_id.to_string()) {
                continue;
            }

            let children = self.store.get_summary_children(&current_id)?;

            for child in children {
                match child {
                    SummaryChild::Messages { ids } => {
                        messages.extend(ids);
                    }
                    SummaryChild::Summaries { ids } => {
                        for id in ids {
                            queue.push_back(id);
                        }
                    }
                }
            }
        }

        // Deduplicate while preserving order.
        let mut seen = HashSet::new();
        messages.retain(|id| seen.insert(id.to_string()));

        Ok(messages)
    }

    /// Get the depth of the DAG from a summary node.
    ///
    /// Depth 0 means the node has no children (shouldn't happen).
    /// Depth 1 means a leaf summary pointing to messages.
    /// Depth N means N levels of condensed summaries.
    pub fn get_depth(&self, summary_id: &SummaryId) -> Result<u32, LcmError> {
        let mut max_depth = 0u32;
        let mut visited = HashSet::new();
        self.dfs_depth(summary_id, 1, &mut max_depth, &mut visited)?;
        Ok(max_depth)
    }

    fn dfs_depth(
        &self,
        summary_id: &SummaryId,
        current_depth: u32,
        max_depth: &mut u32,
        visited: &mut HashSet<String>,
    ) -> Result<(), LcmError> {
        if !visited.insert(summary_id.to_string()) {
            return Ok(());
        }

        if current_depth > *max_depth {
            *max_depth = current_depth;
        }

        let children = self.store.get_summary_children(summary_id)?;

        for child in children {
            match child {
                SummaryChild::Summaries { ids } => {
                    for id in ids {
                        self.dfs_depth(&id, current_depth + 1, max_depth, visited)?;
                    }
                }
                SummaryChild::Messages { .. } => {
                    // Leaf reached — depth stays at current_depth + 1.
                    if current_depth + 1 > *max_depth {
                        *max_depth = current_depth + 1;
                    }
                }
            }
        }

        Ok(())
    }

    // ── Aggregation ────────────────────────────────────────────────────

    /// Aggregate all file references from a summary node and its descendants.
    ///
    /// This is used during compaction to propagate file awareness upward
    /// through the DAG so the model retains knowledge of files encountered
    /// earlier in the session, even after multiple rounds of summarization.
    pub fn aggregate_file_refs(
        &self,
        summary_id: &SummaryId,
    ) -> Result<HashSet<LcmId>, LcmError> {
        let mut file_refs = HashSet::new();
        let mut visited = HashSet::new();

        // Get the summary itself.
        if let Some(summary) = self.store.get_summary(summary_id)? {
            for fr in &summary.file_refs {
                file_refs.insert(fr.clone());
            }
        }

        // Traverse children.
        let children = self.store.get_summary_children(summary_id)?;
        for child in children {
            match child {
                SummaryChild::Messages { ids } => {
                    for msg_id in ids {
                        if let Some(msg) = self.store.get_message(&msg_id)? {
                            // Messages don't have file_refs directly, but
                            // we could derive them from tool outputs.
                            // For now, skip.
                            let _ = msg;
                        }
                    }
                }
                SummaryChild::Summaries { ids } => {
                    for id in ids {
                        if visited.insert(id.to_string()) {
                            let sub_refs = self.aggregate_file_refs(&id)?;
                            file_refs.extend(sub_refs);
                        }
                    }
                }
            }
        }

        Ok(file_refs)
    }

    // ── Validation ─────────────────────────────────────────────────────

    /// Validate the DAG for a conversation.
    ///
    /// Checks for:
    /// - Cycle detection
    /// - Orphaned references
    /// - Referential integrity
    pub fn validate(&self, conversation_id: &str) -> Result<Vec<String>, LcmError> {
        let mut warnings = Vec::new();

        // Check for cycles by doing DFS from each root summary.
        let all_summaries = self.get_all_summaries_for_conv(conversation_id)?;

        for summary in &all_summaries {
            let mut visited = HashSet::new();
            let mut in_stack = HashSet::new();

            if self.detect_cycle(&summary.id, &mut visited, &mut in_stack)? {
                warnings.push(format!("Cycle detected involving summary {}", summary.id));
            }
        }

        // Check for orphaned covered_by references.
        let messages = self
            .store
            .get_conversation_messages(conversation_id)?;
        for msg in &messages {
            if let Some(ref sid) = msg.covered_by {
                if self.store.get_summary(sid)?.is_none() {
                    warnings.push(format!(
                        "Message {} references non-existent summary {}",
                        msg.id, sid
                    ));
                }
            }
        }

        Ok(warnings)
    }

    fn get_all_summaries_for_conv(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<SummaryNode>, LcmError> {
        self.store.get_conversation_summaries(conversation_id)
    }

    fn detect_cycle(
        &self,
        summary_id: &SummaryId,
        visited: &mut HashSet<String>,
        in_stack: &mut HashSet<String>,
    ) -> Result<bool, LcmError> {
        let sid_str = summary_id.to_string();

        if in_stack.contains(&sid_str) {
            return Ok(true); // Cycle detected.
        }

        if visited.contains(&sid_str) {
            return Ok(false); // Already processed.
        }

        visited.insert(sid_str.clone());
        in_stack.insert(sid_str.clone());

        let children = self.store.get_summary_children(summary_id)?;

        for child in children {
            if let SummaryChild::Summaries { ids } = child {
                for id in ids {
                    if self.detect_cycle(&id, visited, in_stack)? {
                        return Ok(true);
                    }
                }
            }
        }

        in_stack.remove(&sid_str);
        Ok(false)
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::types::LcmConfig;

    fn make_store() -> Arc<LcmStore> {
        let store = LcmStore::open_in_memory(LcmConfig::default())
            .expect("Failed to create in-memory store");
        Arc::new(store)
    }

    fn make_message(id: &str, content: &str, ts: i64) -> StoredMessage {
        StoredMessage::new(
            MessageId::from(id),
            "test-conv",
            crate::lcm::types::MessageRole::User,
            content,
            crate::lcm::types::estimate_tokens(content),
            ts,
        )
    }

    fn make_summary(id: &str, text: &str, kind: SummaryKind, level: u8) -> SummaryNode {
        SummaryNode {
            id: SummaryId::from(id),
            conversation_id: "test-conv".to_string(),
            kind,
            text: text.to_string(),
            token_count: crate::lcm::types::estimate_tokens(text),
            created_at_unix_ms: 1000,
            compaction_level: level,
            parents: Vec::new(),
            file_refs: Vec::new(),
        }
    }

    #[test]
    fn test_create_leaf_summary() {
        let store = make_store();
        let dag = SummaryDag::new(store.clone());

        let msg1 = make_message("msg-1", "Hello world", 1000);
        let msg2 = make_message("msg-2", "How are you?", 2000);

        store.persist_message(&msg1).unwrap();
        store.persist_message(&msg2).unwrap();

        let summary = make_summary("sum-1", "User greeted and asked a question", SummaryKind::Leaf, 1);

        dag.create_leaf_summary(&summary, &[msg1.id.clone(), msg2.id.clone()])
            .unwrap();

        // Verify messages are marked as covered.
        let msg1_retrieved = store.get_message(&msg1.id).unwrap().unwrap();
        assert_eq!(msg1_retrieved.covered_by, Some(summary.id.clone()));

        let msg2_retrieved = store.get_message(&msg2.id).unwrap().unwrap();
        assert_eq!(msg2_retrieved.covered_by, Some(summary.id.clone()));

        // Verify DAG children.
        let children = store.get_summary_children(&summary.id).unwrap();
        assert_eq!(children.len(), 1);

        if let SummaryChild::Messages { ids } = &children[0] {
            assert!(ids.contains(&msg1.id));
            assert!(ids.contains(&msg2.id));
        } else {
            panic!("Expected Messages child");
        }
    }

    #[test]
    fn test_create_condensed_summary() {
        let store = make_store();
        let dag = SummaryDag::new(store.clone());

        // Create two leaf summaries first.
        let leaf1 = make_summary("leaf-1", "Summary of first batch", SummaryKind::Leaf, 1);
        let leaf2 = make_summary("leaf-2", "Summary of second batch", SummaryKind::Leaf, 1);

        store.insert_summary(&leaf1).unwrap();
        store.insert_summary(&leaf2).unwrap();

        // Now create a condensed summary.
        let condensed = make_summary(
            "cond-1",
            "Condensed summary of both batches",
            SummaryKind::Condensed,
            2,
        );

        dag.create_condensed_summary(&condensed, &[leaf1.id.clone(), leaf2.id.clone()])
            .unwrap();

        // Verify children.
        let children = store.get_summary_children(&condensed.id).unwrap();
        assert_eq!(children.len(), 1);

        if let SummaryChild::Summaries { ids } = &children[0] {
            assert!(ids.contains(&leaf1.id));
            assert!(ids.contains(&leaf2.id));
        } else {
            panic!("Expected Summaries child");
        }
    }

    #[test]
    fn test_get_descendant_messages() {
        let store = make_store();
        let dag = SummaryDag::new(store.clone());

        // Messages → Leaf → Condensed
        let msg1 = make_message("msg-1", "Message one", 1000);
        let msg2 = make_message("msg-2", "Message two", 2000);
        store.persist_message(&msg1).unwrap();
        store.persist_message(&msg2).unwrap();

        let leaf = make_summary("leaf-1", "Leaf summary", SummaryKind::Leaf, 1);
        dag.create_leaf_summary(&leaf, &[msg1.id.clone(), msg2.id.clone()])
            .unwrap();

        // Should get both messages from leaf.
        let descendants = dag.get_descendant_messages(&leaf.id).unwrap();
        assert_eq!(descendants.len(), 2);
    }

    #[test]
    fn test_detect_no_cycle() {
        let store = make_store();
        let dag = SummaryDag::new(store.clone());

        let leaf = make_summary("leaf-1", "A leaf", SummaryKind::Leaf, 1);
        store.insert_summary(&leaf).unwrap();

        let mut visited = HashSet::new();
        let mut in_stack = HashSet::new();

        let has_cycle = dag.detect_cycle(&leaf.id, &mut visited, &mut in_stack).unwrap();
        assert!(!has_cycle);
    }
}
