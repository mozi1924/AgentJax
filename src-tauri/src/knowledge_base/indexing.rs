//! Per-KB indexing guard — prevents duplicate indexing threads and supports
//! cancellation when a KB is deleted or the user manually stops indexing.
//!
//! ## Usage
//!
//! ```ignore
//! let token = KbIndexingGuard::acquire("my_kb")?;
//! // ... long indexing work, periodically checking token.is_cancelled() ...
//! KbIndexingGuard::release("my_kb");
//! ```

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex};

// ── Indexing Token ──────────────────────────────────────────────────────────

/// A lightweight cancellation token for indexing operations.
#[derive(Clone)]
pub struct IndexingToken {
    cancelled: Arc<AtomicBool>,
}

impl IndexingToken {
    /// Check whether cancellation has been requested.
    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Relaxed)
    }
}

// ── Global Registry ─────────────────────────────────────────────────────────

static GUARD: LazyLock<Mutex<HashMap<String, IndexingToken>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ── Public API ──────────────────────────────────────────────────────────────

/// Try to acquire the indexing lock for a knowledge base.
///
/// Returns an `IndexingToken` if the lock was acquired successfully.
/// Returns `Err(message)` if another indexing operation is already in
/// progress for this KB.
pub fn acquire(kb_id: &str) -> Result<IndexingToken, String> {
    let mut guard = GUARD
        .lock()
        .map_err(|_| "Indexing guard is poisoned".to_string())?;

    if guard.contains_key(kb_id) {
        return Err(format!(
            "Knowledge base '{}' is already being indexed",
            kb_id
        ));
    }

    let token = IndexingToken {
        cancelled: Arc::new(AtomicBool::new(false)),
    };
    guard.insert(kb_id.to_string(), token.clone());
    Ok(token)
}

/// Release the indexing lock for a knowledge base without cancelling.
///
/// Call this when indexing completes successfully or errors out.
pub fn release(kb_id: &str) {
    if let Ok(mut guard) = GUARD.lock() {
        guard.remove(kb_id);
    }
}

/// Cancel an active indexing operation and release the lock.
///
/// Returns `true` if an operation was actually cancelled, `false` if
/// no indexing was in progress for this KB.
pub fn cancel(kb_id: &str) -> bool {
    if let Ok(mut guard) = GUARD.lock() {
        if let Some(token) = guard.remove(kb_id) {
            token.cancelled.store(true, Ordering::Relaxed);
            return true;
        }
    }
    false
}

/// Check whether a KB is currently being indexed.
#[allow(dead_code)]
pub fn is_indexing(kb_id: &str) -> bool {
    GUARD
        .lock()
        .map(|guard| guard.contains_key(kb_id))
        .unwrap_or(false)
}

/// List all KB IDs that are currently being indexed.
pub fn active_indexing_kb_ids() -> Vec<String> {
    GUARD
        .lock()
        .map(|guard| guard.keys().cloned().collect())
        .unwrap_or_default()
}

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_acquire_release() {
        let kb_id = "test-kb-lock";
        let token = acquire(kb_id).expect("first acquire should succeed");
        assert!(is_indexing(kb_id));

        // Second acquire should fail.
        let second = acquire(kb_id);
        assert!(second.is_err());

        release(kb_id);
        assert!(!is_indexing(kb_id));

        // After release, can acquire again.
        drop(token);
        let third = acquire(kb_id);
        assert!(third.is_ok());
        release(kb_id);
    }

    #[test]
    fn test_cancel_sets_token() {
        let kb_id = "test-kb-cancel";
        let token = acquire(kb_id).expect("acquire should succeed");
        assert!(!token.is_cancelled());

        let did_cancel = cancel(kb_id);
        assert!(did_cancel);
        assert!(token.is_cancelled());
        assert!(!is_indexing(kb_id));
    }

    #[test]
    fn test_cancel_noop_when_not_indexing() {
        let did_cancel = cancel("nonexistent-kb");
        assert!(!did_cancel);
    }

    #[test]
    fn test_active_list() {
        let a = acquire("kb-a").unwrap();
        let b = acquire("kb-b").unwrap();
        let mut ids = active_indexing_kb_ids();
        ids.sort();
        assert_eq!(ids, vec!["kb-a", "kb-b"]);

        release("kb-a");
        release("kb-b");
        drop(a);
        drop(b);
        assert!(active_indexing_kb_ids().is_empty());
    }
}
