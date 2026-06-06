//! StreetManager — process-wide registry for the Street notification queue.
//!
//! Follows the exact same pattern as `SubAgentManager` and `background_jobs`:
//! `OnceLock<Mutex<HashMap<conv_id, Vec<Arc<Mutex<StreetItem>>>>>>` static registry.
//!
//! Items are conversation-scoped. On deposit, an event is sent to the
//! conversation's event channel (for frontend auto-trigger). On the next turn,
//! items are collected, formatted, and injected into the system prefix.

use crate::street::types::{StreetEvent, StreetItem, StreetItemStatus, StreetSnapshot};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokio::sync::mpsc;

// ── Constants ───────────────────────────────────────────────────────────────

const MAX_ITEMS_PER_CONVERSATION: usize = 100;

// ── Global Registry ─────────────────────────────────────────────────────────

/// Registry: conversation_id → Vec<Arc<Mutex<StreetItem>>>
type StreetItemMap = std::collections::HashMap<String, Vec<Arc<Mutex<StreetItem>>>>;
static STREET_ITEMS: OnceLock<Mutex<StreetItemMap>> = OnceLock::new();

/// Event channels: conversation_id → mpsc::Sender
static STREET_CHANNELS: OnceLock<Mutex<HashMap<String, mpsc::UnboundedSender<StreetEvent>>>> =
    OnceLock::new();

/// Notifiers: conversation_id → Arc<tokio::sync::Notify>
static STREET_NOTIFIERS: OnceLock<Mutex<HashMap<String, Arc<tokio::sync::Notify>>>> =
    OnceLock::new();

fn registry() -> &'static Mutex<HashMap<String, Vec<Arc<Mutex<StreetItem>>>>> {
    STREET_ITEMS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn channels() -> &'static Mutex<HashMap<String, mpsc::UnboundedSender<StreetEvent>>> {
    STREET_CHANNELS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn notifiers() -> &'static Mutex<HashMap<String, Arc<tokio::sync::Notify>>> {
    STREET_NOTIFIERS.get_or_init(|| Mutex::new(HashMap::new()))
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
                    .min_by_key(|(_, i)| i.lock().ok().map(|i| i.timestamp).unwrap_or(i64::MAX))
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

        // Notify any waiters for this conversation.
        let notifier = Self::get_or_create_notifier(&conv_id);
        notifier.notify_one();
    }

    /// Get or create the Notify object for a conversation.
    pub fn get_or_create_notifier(conversation_id: &str) -> Arc<tokio::sync::Notify> {
        let mut guard = notifiers()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard
            .entry(conversation_id.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Notify::new()))
            .clone()
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
                if let Ok(mut i) = item.lock()
                    && i.status == StreetItemStatus::Pending
                {
                    i.status = StreetItemStatus::Delivered;
                    count += 1;
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
                if let Ok(mut i) = item.lock()
                    && i.id == item_id
                    && i.status == StreetItemStatus::Pending
                {
                    i.status = StreetItemStatus::Dismissed;
                    return true;
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

    /// Check if the conversation should auto-resume because sub-agents are done and Street results are pending.
    pub fn is_auto_resume(conversation_id: &str) -> bool {
        // 1. Any active non-memory sub-agents?
        let has_active_subagents =
            crate::sub_agents::manager::SubAgentManager::list(Some(conversation_id))
                .iter()
                .any(|s| {
                    s.subagent_type != crate::sub_agents::types::SubAgentType::Memory.as_str()
                        && (s.status == crate::sub_agents::types::SubAgentStatus::Running.as_str()
                            || s.status
                                == crate::sub_agents::types::SubAgentStatus::Pending.as_str())
                });

        if has_active_subagents {
            return false;
        }

        // 2. Are there pending street notifications?
        Self::get_pending_count(conversation_id) > 0
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
    #[cfg(test)]
    pub fn cleanup_conversation(conversation_id: &str) -> usize {
        let mut guard = registry()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let removed = guard.remove(conversation_id).map(|v| v.len()).unwrap_or(0);
        // Unregister event channel.
        let mut n_guard = notifiers().lock().unwrap_or_else(|p| p.into_inner());
        n_guard.remove(conversation_id);
        removed
    }

    // ── Event channel management ─────────────────────────────────────────

    /// Register or replace the event channel sender for a conversation.
    pub fn register_event_channel(conversation_id: &str, tx: mpsc::UnboundedSender<StreetEvent>) {
        let mut guard = channels()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.insert(conversation_id.to_string(), tx);
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
    use crate::street::types::Priority;
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

    #[tokio::test]
    async fn test_is_auto_resume() {
        let conv = unique_conv();

        // Initially, no pending street items and no subagents.
        assert!(!StreetManager::is_auto_resume(&conv));

        // Deposit a street item.
        StreetManager::deposit(StreetItem::new(
            &conv,
            crate::street::types::StreetSource::SubAgent,
            Priority::Normal,
            "A test item",
            json!({}),
        ));

        // Now pending count > 0, and no sub-agents.
        assert!(StreetManager::is_auto_resume(&conv));

        // Register a pending sub-agent.
        let spec = crate::sub_agents::types::SubAgentSpec {
            agent_id: format!("test-agent-{}", uuid::Uuid::new_v4().simple()),
            parent_conversation_id: conv.clone(),
            subagent_type: crate::sub_agents::types::SubAgentType::Explore,
            prompt: "Test".to_string(),
            delegated_scope: vec![],
            kept_work: vec![],
            max_turns: 5,
            max_retries: 0,
            use_worktree: false,
            model_id: None,
            parent_request_id: "req_test".to_string(),
            persistent: false,
        };
        let task = crate::sub_agents::manager::SubAgentManager::register(spec);

        // Since there is a pending sub-agent, is_auto_resume should be false.
        assert!(!StreetManager::is_auto_resume(&conv));

        // Mark it as running.
        crate::sub_agents::manager::SubAgentManager::mark_running(&task, tokio::spawn(async {}));
        assert!(!StreetManager::is_auto_resume(&conv));

        // Complete the sub-agent.
        crate::sub_agents::manager::SubAgentManager::complete(&task, json!({}));

        // Sub-agent is now completed (terminal), and street results are pending.
        assert!(StreetManager::is_auto_resume(&conv));
    }
}
