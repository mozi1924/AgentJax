//! Street persistence — JSONL-backed notification store.
//!
//! Notifications are persisted per-conversation as JSONL files alongside the
//! conversation's existing artifacts (`metadata.json`, `messages.jsonl`, `lcm.db`):
//!
//! `~/.agentjax/agents/{agent_id}/sessions/{conv_id}/notifications.jsonl`
//!
//! This enables full traceability of async task results across app restarts.
//!
//! ## Storage strategy
//!
//! - **deposit**: append a single line to the JSONL file (O(1))
//! - **status change** (delivered/dismissed): rewrite the entire file (O(n))
//!   since item count per conversation is bounded (~100). Rewriting avoids
//!   the complexity of append-only status updates.
//! - **load**: read all lines, deduplicate by id (later status wins on
//!   restart), hydrate into `Arc<Mutex<StreetItem>>`.
//!
//! ## File format (JSONL)
//!
//! Each line is a full `StreetItem` JSON object. The file is append-only
//! for deposits; status changes rewrite the full current state.

use crate::conversation_store::conversation_dir_path;
use crate::error::AgentJaxResult;
use crate::jsonl_store;
use crate::street::types::StreetItem;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use crate::conversation_store::paths::NOTIFICATIONS_FILE_NAME;

/// Resolve the path to the JSONL file for a given conversation.
///
/// The file lives alongside `metadata.json`, `messages.jsonl`, and `lcm.db`
/// in the conversation's session directory:
///
/// `~/.agentjax/agents/{agent_id}/sessions/{conv_id}/notifications.jsonl`
///
/// Uses `DEFAULT_AGENT_ID` ("main") as the default agent scope. If multi-agent
/// support is needed in the future, the agent_id should be added to `StreetItem`.
pub fn notification_path(conversation_id: &str) -> AgentJaxResult<PathBuf> {
    let dir = conversation_dir_path(
        crate::config::constants::DEFAULT_AGENT_ID,
        conversation_id,
    )?;
    Ok(dir.join(NOTIFICATIONS_FILE_NAME))
}

/// Load all persisted StreetItems for a conversation from disk.
///
/// Returns an empty vec if the file doesn't exist or is empty.
/// On load, items with duplicate IDs are deduplicated: the **last**
/// occurrence wins, so a rewritten file with updated statuses takes
/// effect correctly.
pub fn load_items(conversation_id: &str) -> AgentJaxResult<Vec<Arc<Mutex<StreetItem>>>> {
    let path = notification_path(conversation_id)?;
    let items: Vec<StreetItem> = jsonl_store::read_jsonl(&path, "notification")?;

    // Deduplicate by id (last occurrence wins, insertion order preserved).
    let deduped = jsonl_store::dedup_vec(items, |item| item.id.clone());

    Ok(deduped
        .into_iter()
        .map(|item| Arc::new(Mutex::new(item)))
        .collect())
}

/// Persist a single item by appending to the conversation's JSONL file.
///
/// This is called on every `deposit()`. The file is opened in append mode
/// and a single JSON line is written.
pub fn append_item(item: &StreetItem) -> AgentJaxResult<()> {
    let path = notification_path(&item.conversation_id)?;
    jsonl_store::append_line(&path, item, "notification")?;
    Ok(())
}

/// Rewrite the full set of items for a conversation to disk.
///
/// Called when items are delivered, dismissed, or pruned — any operation
/// that changes status or removes items. The full vec is serialized as
/// JSONL (one line per item), replacing the file entirely.
pub fn save_items(conversation_id: &str, items: &[Arc<Mutex<StreetItem>>]) -> AgentJaxResult<()> {
    let path = notification_path(conversation_id)?;

    if items.is_empty() {
        return jsonl_store::remove_file(&path, "notification");
    }

    let owned: Vec<StreetItem> = items
        .iter()
        .map(|arc| arc.lock().unwrap_or_else(|p| p.into_inner()).clone())
        .collect();

    jsonl_store::write_lines(&path, &owned, "notification")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::street::types::{Priority, StreetSource, StreetItemStatus};
    use serde_json::json;
    use std::sync::Mutex;

    fn unique_conv() -> String {
        format!("persist-test-{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn test_roundtrip_empty() {
        let conv = unique_conv();
        let items = load_items(&conv).unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn test_append_and_load() {
        let conv = unique_conv();
        let item = StreetItem::new(
            &conv,
            StreetSource::SubAgent,
            Priority::Normal,
            "Test notification",
            json!({"key": "value"}),
        );
        append_item(&item).unwrap();

        let loaded = load_items(&conv).unwrap();
        assert_eq!(loaded.len(), 1);
        let loaded_item = loaded[0].lock().unwrap();
        assert_eq!(loaded_item.title, "Test notification");
        assert_eq!(loaded_item.source, StreetSource::SubAgent);
        assert_eq!(loaded_item.status, StreetItemStatus::Pending);
    }

    #[test]
    fn test_save_items_rewrites_file() {
        let conv = unique_conv();
        let item1 = Arc::new(Mutex::new(StreetItem::new(
            &conv,
            StreetSource::SubAgent,
            Priority::Normal,
            "First",
            json!({}),
        )));
        let item2 = Arc::new(Mutex::new(StreetItem::new(
            &conv,
            StreetSource::BackgroundJob,
            Priority::Low,
            "Second",
            json!({}),
        )));

        // Save both.
        save_items(&conv, &[item1.clone(), item2.clone()]).unwrap();
        assert_eq!(load_items(&conv).unwrap().len(), 2);

        // Save only one (simulating a removal/prune).
        save_items(&conv, std::slice::from_ref(&item1)).unwrap();
        let loaded = load_items(&conv).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].lock().unwrap().title, "First");
    }

    #[test]
    fn test_preserves_insertion_order_on_load() {
        let conv = unique_conv();
        let item_a = StreetItem::new(&conv, StreetSource::SubAgent, Priority::High, "A", json!({}));
        let item_b = StreetItem::new(
            &conv,
            StreetSource::BackgroundJob,
            Priority::Low,
            "B",
            json!({}),
        );
        append_item(&item_a).unwrap();
        append_item(&item_b).unwrap();

        let loaded = load_items(&conv).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].lock().unwrap().title, "A");
        assert_eq!(loaded[1].lock().unwrap().title, "B");
    }

    #[test]
    fn test_deduplicate_by_id() {
        let conv = unique_conv();

        // Create two items with same ID (simulating a rewrite after status change).
        let mut item = StreetItem::new(
            &conv,
            StreetSource::SubAgent,
            Priority::Normal,
            "Original",
            json!({}),
        );
        let _same_id = item.id.clone();
        append_item(&item).unwrap();

        // Now mark as delivered by rewriting with updated status.
        item.status = StreetItemStatus::Delivered;
        // Manually rewrite (simulating what save_items does).
        let arc_item = Arc::new(Mutex::new(item));
        save_items(&conv, &[arc_item]).unwrap();

        let loaded = load_items(&conv).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].lock().unwrap().status, StreetItemStatus::Delivered);
    }
}
