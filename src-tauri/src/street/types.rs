//! Street notification queue system — types.
//!
//! These types define the core data structures for the Street:
//! async work results that are proactively delivered to the main agent.

use serde::{Deserialize, Serialize};
use serde_json::Value;

// ── Street Source ──────────────────────────────────────────────────────────

/// Where a Street notification originated.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreetSource {
    /// Result from an async sub-agent.
    SubAgent,
    /// Result from a background tool job.
    BackgroundJob,
    /// Notification from the memory sub-agent.
    MemoryAgent,
    /// System-level event (MCP discovery, config change, etc.).
    System,
    /// External event (user correction from outside the chat, etc.).
    External,
}

impl StreetSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreetSource::SubAgent => "subAgent",
            StreetSource::BackgroundJob => "backgroundJob",
            StreetSource::MemoryAgent => "memoryAgent",
            StreetSource::System => "system",
            StreetSource::External => "external",
        }
    }
}

// ── Priority ───────────────────────────────────────────────────────────────

/// Priority of a Street notification. Derives Ord so Urgent > High > Normal > Low.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "camelCase")]
pub enum Priority {
    Low,
    Normal,
    High,
    Urgent,
}

impl Priority {
    pub fn as_str(&self) -> &'static str {
        match self {
            Priority::Low => "low",
            Priority::Normal => "normal",
            Priority::High => "high",
            Priority::Urgent => "urgent",
        }
    }
}

// ── Street Item Status ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum StreetItemStatus {
    /// Not yet delivered to the agent.
    Pending,
    /// Delivered to the agent via context injection.
    Delivered,
    /// Dismissed by the user.
    Dismissed,
}

impl StreetItemStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            StreetItemStatus::Pending => "pending",
            StreetItemStatus::Delivered => "delivered",
            StreetItemStatus::Dismissed => "dismissed",
        }
    }

    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            StreetItemStatus::Delivered | StreetItemStatus::Dismissed
        )
    }
}

// ── Street Item ─────────────────────────────────────────────────────────────

/// A single notification in the Street queue.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreetItem {
    pub id: String,
    pub source: StreetSource,
    pub priority: Priority,
    pub title: String,
    pub payload: Value,
    pub timestamp: i64,
    pub status: StreetItemStatus,
    pub conversation_id: String,
}

impl StreetItem {
    /// Create a new StreetItem with a UUID id, current timestamp, and Pending status.
    pub fn new(
        conversation_id: &str,
        source: StreetSource,
        priority: Priority,
        title: &str,
        payload: Value,
    ) -> Self {
        Self {
            id: format!("street-{}", uuid::Uuid::new_v4().simple()),
            source,
            priority,
            title: title.to_string(),
            payload,
            timestamp: crate::conversation_store_utils::now_unix_ms(),
            status: StreetItemStatus::Pending,
            conversation_id: conversation_id.to_string(),
        }
    }
}

// ── Street Event ────────────────────────────────────────────────────────────

/// Events emitted through the Street event channel to the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StreetEvent {
    /// A new item was deposited into the Street.
    Deposited {
        conversation_id: String,
        item_id: String,
        title: String,
        priority: Priority,
    },
    /// Items were delivered (injected into context).
    Cleared {
        conversation_id: String,
        count: usize,
    },
}

// ── Street Snapshot (for Tauri IPC) ────────────────────────────────────────

/// A lightweight, serializable snapshot of a Street item for the frontend.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StreetSnapshot {
    pub id: String,
    pub source: String,
    pub priority: String,
    pub title: String,
    pub timestamp: i64,
    pub status: String,
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_priority_ordering() {
        assert!(Priority::Urgent > Priority::High);
        assert!(Priority::High > Priority::Normal);
        assert!(Priority::Normal > Priority::Low);
    }

    #[test]
    fn test_street_item_new() {
        let item = StreetItem::new(
            "conv-1",
            StreetSource::SubAgent,
            Priority::Normal,
            "Test notification",
            serde_json::json!({"ok": true}),
        );
        assert!(item.id.starts_with("street-"));
        assert_eq!(item.source, StreetSource::SubAgent);
        assert_eq!(item.status, StreetItemStatus::Pending);
        assert!(item.timestamp > 0);
    }

    #[test]
    fn test_status_terminal() {
        assert!(StreetItemStatus::Delivered.is_terminal());
        assert!(StreetItemStatus::Dismissed.is_terminal());
        assert!(!StreetItemStatus::Pending.is_terminal());
    }
}
