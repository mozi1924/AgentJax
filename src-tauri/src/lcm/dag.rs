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
#[cfg(test)]
use crate::lcm::types::StoredMessage;
use crate::lcm::types::{LcmError, MessageId, SummaryChild, SummaryId, SummaryKind, SummaryNode};
#[cfg(test)]
use std::collections::HashSet;
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
}

// ── Test-only helpers ─────────────────────────────────────────────────────────

/// Get all descendant message IDs for a summary node via BFS.
/// Only available in test builds.
#[cfg(test)]
pub fn get_descendant_messages(
    dag: &SummaryDag,
    summary_id: &SummaryId,
) -> Result<Vec<MessageId>, LcmError> {
    use std::collections::VecDeque;
    let mut messages = Vec::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    queue.push_back(summary_id.clone());

    while let Some(current_id) = queue.pop_front() {
        if !visited.insert(current_id.to_string()) {
            continue;
        }

        let children = dag.store.get_summary_children(&current_id)?;

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

    let mut seen = HashSet::new();
    messages.retain(|id| seen.insert(id.to_string()));
    Ok(messages)
}

/// Detect cycles in the DAG via DFS. Only available in test builds.
#[cfg(test)]
fn detect_cycle(
    dag: &SummaryDag,
    summary_id: &SummaryId,
    visited: &mut HashSet<String>,
    in_stack: &mut HashSet<String>,
) -> Result<bool, LcmError> {
    let sid_str = summary_id.to_string();

    if in_stack.contains(&sid_str) {
        return Ok(true);
    }
    if visited.contains(&sid_str) {
        return Ok(false);
    }

    visited.insert(sid_str.clone());
    in_stack.insert(sid_str.clone());

    let children = dag.store.get_summary_children(summary_id)?;
    for child in children {
        if let SummaryChild::Summaries { ids } = child {
            for id in ids {
                if detect_cycle(dag, &id, visited, in_stack)? {
                    return Ok(true);
                }
            }
        }
    }

    in_stack.remove(&sid_str);
    Ok(false)
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
            0,
            0,
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

        let summary = make_summary(
            "sum-1",
            "User greeted and asked a question",
            SummaryKind::Leaf,
            1,
        );

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
        let descendants = get_descendant_messages(&dag, &leaf.id).unwrap();
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

        let has_cycle = detect_cycle(&dag, &leaf.id, &mut visited, &mut in_stack).unwrap();
        assert!(!has_cycle);
    }
}
