//! StreetManager — process-wide registry for the Street notification queue.
//!
//! Follows the exact same pattern as `SubAgentManager` and `background_jobs`:
//! `OnceLock<Mutex<HashMap<conv_id, Vec<Arc<Mutex<StreetItem>>>>>>` static registry.
//!
//! Items are conversation-scoped. On deposit, an event is sent to the
//! conversation's event channel (for frontend auto-trigger). On the next turn,
//! items are collected, formatted, and injected into the developer prefix.

use crate::conversation_store_utils::now_unix_ms;
use crate::street::types::{
    Priority, StreetEvent, StreetItem, StreetItemStatus, StreetSnapshot,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

// ── Constants ───────────────────────────────────────────────────────────────

const MAX_ITEMS_PER_CONVERSATION: usize = 100;
const TERMINAL_ITEM_RETENTION_MS: i64 = 60 * 60 * 1_000; // 1 hour
const MAX_RETAINED_TERMINAL_ITEMS: usize = 200;

// ── Global Registry ─────────────────────────────────────────────────────────

/// Registry: conversation_id → Vec<Arc<Mutex<StreetItem>>>
static STREET_ITEMS: OnceLock<Mutex<HashMap<String, Vec<Arc<Mutex<StreetItem>>>>>> =
    OnceLock::new();

/// Event channels: conversation_id → mpsc::Sender
static STREET_CHANNELS: OnceLock<Mutex<HashMap<String, mpsc::UnboundedSender<StreetEvent>>>> =
    OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Vec<Arc<Mutex<StreetItem>>>>> {
    STREET_ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn channels() -> &'static Mutex<HashMap<String, mpsc::UnboundedSender<StreetEvent>>> {
    STREET_CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

// ── Pruning ─────────────────────────────────────────────────────────────────

fn prune_terminal_items(
    items: &mut Vec<Arc<Mutex<StreetItem>>>,
    retention_ms: i64,
    max_retained: usize,
) -> usize {
    let now = now_unix_ms();
    let cutoff = now.saturating_sub(retention_ms);
    let mut removed = 0usize;

    // Remove expired terminal items.
    items.retain(|item| {
        if let Ok(i) = item.lock() {
            if i.status.is_terminal() && i.timestamp <= cutoff {
                removed += 1;
                return false;
            }
        }
        true
    });

    // If still over max, remove oldest terminal items.
    let terminal_count = items
        .iter()
        .filter(|item| {
            item.lock()
                .ok()
                .map(|i| i.status.is_terminal())
                .unwrap_or(false)
        })
        .count();

    if terminal_count > max_retained {
        let excess = terminal_count - max_retained;
        let mut to_remove = Vec::new();
        let mut terminal_indices: Vec<(usize, i64)> = items
            .iter()
            .enumerate()
            .filter_map(|(idx, item)| {
                item.lock()
                    .ok()
                    .filter(|i| i.status.is_terminal())
                    .map(|i| (idx, i.timestamp))
            })
            .collect();
        terminal_indices.sort_by_key(|(_, ts)| *ts);
        for (idx, _) in terminal_indices.into_iter().take(excess) {
            to_remove.push(idx);
        }
        // Remove from the end to preserve indices.
        to_remove.sort_unstable_by(|a, b| b.cmp(a));
        for idx in to_remove {
            items.remove(idx);
            removed += 1;
        }
    }

    removed
}

// ── StreetManager ───────────────────────────────────────────────────────────

pub struct StreetManager;

impl StreetManager {
    /// Deposit a notification into the Street queue.
    ///
    /// Items are conversation-scoped. A `StreetEvent::Deposited` is sent
    /// to the conversation's event channel (if one exists) so the frontend
    /// can react (badge update, auto-trigger).
    pub fn deposit(item: StreetItem) {
        let conv_id = item.conversation_id.clone();
        let event = StreetEvent::Deposited {
            conversation_id: conv_id.clone(),
            item_id: item.id.clone(),
            title: item.title.clone(),
            priority: item.priority,
        };

        let mut guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let items = guard.entry(conv_id.clone()).or_default();

        // Enforce per-conversation cap.
        while items.len() >= MAX_ITEMS_PER_CONVERSATION {
            // Remove the oldest terminal item.
            if let Some(pos) = items.iter().position(|i| {
                i.lock()
                    .ok()
                    .map(|i| i.status.is_terminal())
                    .unwrap_or(false)
            }) {
                items.remove(pos);
            } else {
                // All are pending — remove the oldest by timestamp.
                if let Some(pos) = items
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, i)| {
                        i.lock().ok().map(|i| i.timestamp).unwrap_or(i64::MAX)
                    })
                    .map(|(idx, _)| idx)
                {
                    items.remove(pos);
                } else {
                    break;
                }
            }
        }

        items.push(Arc::new(Mutex::new(item)));
        drop(guard);

        // Send event to the conversation's channel.
        Self::send_event(&conv_id, event);
    }

    /// Collect all Pending items for a conversation, sorted by priority
    /// (highest first) then by timestamp (oldest first).
    pub fn collect_pending(conversation_id: &str) -> Vec<StreetItem> {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut pending: Vec<StreetItem> = guard
            .get(conversation_id)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.lock().ok().and_then(|i| {
                            if i.status == StreetItemStatus::Pending {
                                Some(i.clone())
                            } else {
                                None
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // Sort: highest priority first, then oldest first.
        pending.sort_by(|a, b| {
            b.priority
                .cmp(&a.priority)
                .then_with(|| a.timestamp.cmp(&b.timestamp))
        });
        pending
    }

    /// Mark all Pending items for a conversation as Delivered.
    /// Returns the number of items marked.
    pub fn mark_delivered(conversation_id: &str) -> usize {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        let mut count = 0usize;
        if let Some(items) = guard.get(conversation_id) {
            for item in items {
                if let Ok(mut i) = item.lock() {
                    if i.status == StreetItemStatus::Pending {
                        i.status = StreetItemStatus::Delivered;
                        count += 1;
                    }
                }
            }
        }

        if count > 0 {
            drop(guard);
            Self::send_event(
                conversation_id,
                StreetEvent::Cleared {
                    conversation_id: conversation_id.to_string(),
                    count,
                },
            );
        }

        count
    }

    /// Mark a single item as Dismissed.
    pub fn mark_dismissed(item_id: &str, conversation_id: &str) -> bool {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        if let Some(items) = guard.get(conversation_id) {
            for item in items {
                if let Ok(mut i) = item.lock() {
                    if i.id == item_id && i.status == StreetItemStatus::Pending {
                        i.status = StreetItemStatus::Dismissed;
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Get the count of Pending items for a conversation.
    pub fn get_pending_count(conversation_id: &str) -> usize {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        guard
            .get(conversation_id)
            .map(|items| {
                items
                    .iter()
                    .filter(|item| {
                        item.lock()
                            .ok()
                            .map(|i| i.status == StreetItemStatus::Pending)
                            .unwrap_or(false)
                    })
                    .count()
            })
            .unwrap_or(0)
    }

    /// Get snapshots of Pending items for a conversation (for Tauri IPC).
    pub fn get_pending_snapshots(conversation_id: &str) -> Vec<StreetSnapshot> {
        let guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());

        guard
            .get(conversation_id)
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        item.lock().ok().and_then(|i| {
                            if i.status == StreetItemStatus::Pending {
                                Some(StreetSnapshot {
                                    id: i.id.clone(),
                                    source: i.source.as_str().to_string(),
                                    priority: i.priority.as_str().to_string(),
                                    title: i.title.clone(),
                                    timestamp: i.timestamp,
                                    status: i.status.as_str().to_string(),
                                })
                            } else {
                                None
                            }
                        })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Remove all items for a conversation (called on conversation delete).
    pub fn cleanup_conversation(conversation_id: &str) -> usize {
        let mut guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = guard.remove(conversation_id).map(|v| v.len()).unwrap_or(0);
        drop(guard);
        // Unregister event channel.
        Self::unregister_event_channel(conversation_id);
        removed
    }

    // ── Event channel management ─────────────────────────────────────────

    /// Register or replace the event channel sender for a conversation.
    pub fn register_event_channel(
        conversation_id: &str,
        tx: mpsc::UnboundedSender<StreetEvent>,
    ) {
        let mut guard = channels()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(conversation_id.to_string(), tx);
    }

    /// Remove the event channel sender for a conversation.
    pub fn unregister_event_channel(conversation_id: &str) {
        let mut guard = channels()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.remove(conversation_id);
    }

    /// Send a StreetEvent to the conversation's event channel.
    fn send_event(conversation_id: &str, event: StreetEvent) {
        let guard = channels()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(tx) = guard.get(conversation_id) {
            let _ = tx.send(event);
        }
    }
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn unique_conv() -> String {
        format!("street-conv-{}", uuid::Uuid::new_v4().simple())
    }

    #[test]
    fn test_deposit_and_collect_pending() {
        let conv = unique_conv();
        let item = StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "Test notification",
            json!({"ok": true}),
        );
        StreetManager::deposit(item);

        let pending = StreetManager::collect_pending(&conv);
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].title, "Test notification");
    }

    #[test]
    fn test_mark_delivered() {
        let conv = unique_conv();
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "Item 1",
            json!({}),
        ));
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::BackgroundJob,
            Priority::Low,
            "Item 2",
            json!({}),
        ));

        let count = StreetManager::mark_delivered(&conv);
        assert_eq!(count, 2);

        let pending = StreetManager::collect_pending(&conv);
        assert!(pending.is_empty());
    }

    #[test]
    fn test_sort_by_priority() {
        let conv = unique_conv();
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::BackgroundJob,
            Priority::Low,
            "Low pri",
            json!({}),
        ));
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Urgent,
            "Urgent!",
            json!({}),
        ));
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::System,
            Priority::Normal,
            "Normal",
            json!({}),
        ));

        let pending = StreetManager::collect_pending(&conv);
        assert_eq!(pending[0].priority, Priority::Urgent);
        assert_eq!(pending[1].priority, Priority::Normal);
        assert_eq!(pending[2].priority, Priority::Low);
    }

    #[test]
    fn test_mark_dismissed() {
        let conv = unique_conv();
        let item = StreetItem::new(
            &conv,
            crate::street::types::StreetSource::System,
            Priority::Normal,
            "Dismiss me",
            json!({}),
        );
        let item_id = item.id.clone();
        StreetManager::deposit(item);

        assert!(StreetManager::mark_dismissed(&item_id, &conv));
        let pending = StreetManager::collect_pending(&conv);
        assert!(pending.is_empty());
    }

    #[test]
    fn test_get_pending_count() {
        let conv = unique_conv();
        assert_eq!(StreetManager::get_pending_count(&conv), 0);
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "Test",
            json!({}),
        ));
        assert_eq!(StreetManager::get_pending_count(&conv), 1);
    }

    #[test]
    fn test_get_pending_snapshots() {
        let conv = unique_conv();
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "Snapshot test",
            json!({"key": "value"}),
        ));
        let snapshots = StreetManager::get_pending_snapshots(&conv);
        assert_eq!(snapshots.len(), 1);
        assert_eq!(snapshots[0].source, "subAgent");
        assert_eq!(snapshots[0].title, "Snapshot test");
    }

    #[test]
    fn test_cleanup_conversation() {
        let conv = unique_conv();
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "Test",
            json!({}),
        ));
        let removed = StreetManager::cleanup_conversation(&conv);
        assert_eq!(removed, 1);
        assert_eq!(StreetManager::get_pending_count(&conv), 0);
    }

    #[test]
    fn test_conversation_isolation() {
        let ca = unique_conv();
        let cb = unique_conv();
        StreetManager::deposit(StreetItem::new(
            &ca,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "A's item",
            json!({}),
        ));
        StreetManager::deposit(StreetItem::new(
            &cb,
            crate::street::types::StreetSource::System,
            Priority::High,
            "B's item",
            json!({}),
        ));

        assert_eq!(StreetManager::get_pending_count(&ca), 1);
        assert_eq!(StreetManager::get_pending_count(&cb), 1);

        let a_pending = StreetManager::collect_pending(&ca);
        assert_eq!(a_pending[0].title, "A's item");

        let b_pending = StreetManager::collect_pending(&cb);
        assert_eq!(b_pending[0].title, "B's item");
    }
}
