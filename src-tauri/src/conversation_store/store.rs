//! Central `ConversationStore` struct — unifies the previously free-floating
//! conversation state (lock registry, index cache, path resolution) into a
//! single typed handle.
//!
//! The underlying lock and cache registries remain process-global (via
//! `OnceLock<Mutex<...>>`) to ensure thread safety across multiple store
//! instances, but access is routed through this struct for clarity.
//!
//! High-level CRUD operations (append_line, load_conversation, etc.) remain
//! as free functions in `mutations.rs` / `queries.rs`. This struct provides
//! the foundational path, lock, and cache primitives they build upon.
//!
//! NOTE: ConversationStore is reserved for a future API unification pass
//! and is currently unused. Remove `#![allow(dead_code)]` once adopted.
#![allow(dead_code)]

use super::locks;
use super::paths;
use super::types::ConversationSummary;
use crate::error::AgentJaxResult;
use std::collections::HashSet;
use std::path::PathBuf;

/// Central handle for all conversation I/O for a single agent.
///
/// Created once per agent and shared via `Arc<ConversationStore>`.
/// Provides path resolution, lock management, and index cache access.
///
/// Currently unused — the codebase still uses the module-level free functions.
/// This struct is reserved for a future API unification pass.
#[derive(Debug, Clone)]
pub struct ConversationStore {
    agent_id: String,
}

impl ConversationStore {
    pub fn new(agent_id: impl Into<String>) -> Self {
        Self {
            agent_id: agent_id.into(),
        }
    }

    pub fn agent_id(&self) -> &str {
        &self.agent_id
    }

    // ── Path helpers ─────────────────────────────────────────────────────

    pub fn conversations_dir(&self) -> AgentJaxResult<PathBuf> {
        paths::conversations_dir_path(&self.agent_id)
    }

    pub fn ensure_conversations_dir(&self) -> AgentJaxResult<PathBuf> {
        paths::ensure_conversations_dir(&self.agent_id)
    }

    pub fn conversation_dir(&self, conversation_id: &str) -> AgentJaxResult<PathBuf> {
        paths::conversation_dir_path(&self.agent_id, conversation_id)
    }

    pub fn metadata_path(&self, conversation_id: &str) -> AgentJaxResult<PathBuf> {
        paths::conversation_metadata_path(&self.agent_id, conversation_id)
    }

    pub fn messages_path(&self, conversation_id: &str) -> AgentJaxResult<PathBuf> {
        paths::conversation_messages_path(&self.agent_id, conversation_id)
    }

    pub fn workspace_path(&self, conversation_id: &str) -> AgentJaxResult<PathBuf> {
        paths::conversation_workspace_path(&self.agent_id, conversation_id)
    }

    pub fn lcm_db_path(&self, conversation_id: &str) -> AgentJaxResult<PathBuf> {
        paths::conversation_lcm_db_path(&self.agent_id, conversation_id)
    }

    pub fn ensure_session_layout(&self, conversation_id: &str) -> AgentJaxResult<()> {
        paths::ensure_session_layout(&self.agent_id, conversation_id)
    }

    pub fn list_conversation_ids(&self) -> AgentJaxResult<Vec<String>> {
        paths::list_conversation_ids(&self.agent_id)
    }

    // ── Lock helpers ─────────────────────────────────────────────────────

    pub fn with_lock<T, F>(&self, conversation_id: &str, action: F) -> AgentJaxResult<T>
    where
        F: FnOnce() -> AgentJaxResult<T>,
    {
        locks::with_conversation_lock(conversation_id, action)
    }

    // ── Cache helpers ────────────────────────────────────────────────────

    pub fn cached_line_id_exists(
        &self,
        conversation_id: &str,
        line_id: &str,
    ) -> AgentJaxResult<Option<bool>> {
        locks::cached_line_id_exists(conversation_id, line_id)
    }

    pub fn replace_cached_line_ids(
        &self,
        conversation_id: &str,
        line_ids: HashSet<String>,
    ) -> AgentJaxResult<()> {
        locks::replace_cached_line_ids(conversation_id, line_ids)
    }

    pub fn insert_cached_line_id(
        &self,
        conversation_id: &str,
        line_id: &str,
    ) -> AgentJaxResult<()> {
        locks::insert_cached_line_id(conversation_id, line_id)
    }

    pub fn cached_summary(
        &self,
        conversation_id: &str,
    ) -> AgentJaxResult<Option<ConversationSummary>> {
        locks::cached_summary(conversation_id)
    }

    pub fn replace_cached_summary(
        &self,
        conversation_id: &str,
        summary: ConversationSummary,
    ) -> AgentJaxResult<()> {
        locks::replace_cached_summary(conversation_id, summary)
    }

    pub fn invalidate_cached_index(&self, conversation_id: &str) {
        let _ = locks::invalidate_cached_conversation_index(conversation_id);
    }
}

