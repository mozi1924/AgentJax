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
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::street::types::StreetItem;
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

const NOTIFICATIONS_FILE_NAME: &str = "notifications.jsonl";

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

/// Ensure the conversation directory (parent of the notification file) exists.
fn ensure_conversation_dir(conversation_id: &str) -> AgentJaxResult<()> {
    let path = notification_path(conversation_id)?;
    if let Some(parent) = path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to create conversation directory {}: {e}",
                    parent.display()
                ))
            })?;
        }
    }
    Ok(())
}

/// Load all persisted StreetItems for a conversation from disk.
///
/// Returns an empty vec if the file doesn't exist or is empty.
/// On load, items with duplicate IDs are deduplicated: the **last**
/// occurrence wins, so a rewritten file with updated statuses takes
/// effect correctly.
pub fn load_items(conversation_id: &str) -> AgentJaxResult<Vec<Arc<Mutex<StreetItem>>>> {
    let path = notification_path(conversation_id)?;
    if !path.exists() {
        return Ok(Vec::new());
    }

    let content = fs::read_to_string(&path).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to read notifications file {}: {e}",
            path.display()
        ))
    })?;

    let mut seen: std::collections::HashMap<String, Arc<Mutex<StreetItem>>> =
        std::collections::HashMap::new();
    // Track insertion order so we can preserve it.
    let mut order: Vec<String> = Vec::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        match serde_json::from_str::<StreetItem>(line) {
            Ok(item) => {
                let id = item.id.clone();
                if !seen.contains_key(&id) {
                    order.push(id.clone());
                }
                seen.insert(id, Arc::new(Mutex::new(item)));
            }
            Err(e) => {
                log::warn!(
                    "Failed to parse notification line in {}: {e}",
                    path.display()
                );
            }
        }
    }

    // Preserve insertion order.
    let mut items: Vec<Arc<Mutex<StreetItem>>> = Vec::with_capacity(order.len());
    for id in &order {
        if let Some(item) = seen.remove(id) {
            items.push(item);
        }
    }

    Ok(items)
}

/// Check whether a persisted notification file exists for a conversation.
pub fn has_persisted_items(conversation_id: &str) -> bool {
    notification_path(conversation_id)
        .ok()
        .map(|p| p.exists())
        .unwrap_or(false)
}

/// Persist a single item by appending to the conversation's JSONL file.
///
/// This is called on every `deposit()`. The file is opened in append mode
/// and a single JSON line is written.
pub fn append_item(item: &StreetItem) -> AgentJaxResult<()> {
    let conv_id = &item.conversation_id;
    ensure_conversation_dir(conv_id)?;
    let path = notification_path(conv_id)?;

    let line = serde_json::to_string(item).map_err(|e| {
        AgentJaxError::internal(format!("Failed to serialize StreetItem: {e}"))
    })?;

    // Use std::fs::OpenOptions with append, create if not exists.
    use std::io::Write;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| {
            AgentJaxError::internal(format!(
                "Failed to open notifications file {} for append: {e}",
                path.display()
            ))
        })?;

    writeln!(file, "{line}").map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to write to notifications file {}: {e}",
            path.display()
        ))
    })?;

    Ok(())
}

/// Rewrite the full set of items for a conversation to disk.
///
/// Called when items are delivered, dismissed, or pruned — any operation
/// that changes status or removes items. The full vec is serialized as
/// JSONL (one line per item), replacing the file entirely.
///
/// Pass `None` for `items` to clear the file (e.g., when all items for a
/// conversation have been pruned).
pub fn save_items(conversation_id: &str, items: &[Arc<Mutex<StreetItem>>]) -> AgentJaxResult<()> {
    if items.is_empty() {
        // Remove the file if it exists — no items to persist.
        let path = notification_path(conversation_id)?;
        if path.exists() {
            fs::remove_file(&path).map_err(|e| {
                AgentJaxError::internal(format!(
                    "Failed to remove notifications file {}: {e}",
                    path.display()
                ))
            })?;
        }
        return Ok(());
    }

    ensure_conversation_dir(conversation_id)?;
    let path = notification_path(conversation_id)?;

    let mut lines: Vec<String> = Vec::with_capacity(items.len());
    for item_arc in items {
        if let Ok(item) = item_arc.lock() {
            match serde_json::to_string(&*item) {
                Ok(line) => lines.push(line),
                Err(e) => {
                    log::warn!("Failed to serialize StreetItem for save: {e}");
                }
            }
        }
    }

    fs::write(&path, lines.join("\n")).map_err(|e| {
        AgentJaxError::internal(format!(
            "Failed to write notifications file {}: {e}",
            path.display()
        ))
    })?;

    Ok(())
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
        save_items(&conv, &[item1.clone()]).unwrap();
        let loaded = load_items(&conv).unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].lock().unwrap().title, "First");
    }

    #[test]
    fn test_save_empty_removes_file() {
        let conv = unique_conv();
        let item = Arc::new(Mutex::new(StreetItem::new(
            &conv,
            StreetSource::SubAgent,
            Priority::Normal,
            "Temp",
            json!({}),
        )));
        save_items(&conv, &[item]).unwrap();
        assert!(has_persisted_items(&conv));

        save_items(&conv, &[]).unwrap();
        assert!(!has_persisted_items(&conv));
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
        let same_id = item.id.clone();
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
