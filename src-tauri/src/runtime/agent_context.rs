//! Agent context abstraction — pluggable context management for the agent loop.
//!
//! The `AgentContext` trait decouples the agent's tool-calling loop from the
//! specific context storage strategy. This allows different agent types to
//! use different backends:
//!
//! - **`LcmAgentContext`** — Full LCM engine (file-backed SQLite + compaction)
//!   for the main conversation agent.
//! - **`InMemoryContext`** — Simple in-memory message buffer for ephemeral
//!   sub-agents. No disk I/O, no compaction, auto-cleaned on drop.
//! - **`MemoryAgentContext`** — Read-only view of the parent conversation's
//!   recent LCM history. Used by the background memory agent.

use crate::error::AgentJaxResult;
use crate::lcm::types::StoredMessage;
use serde_json::Value;
use std::sync::Arc;

// ── AgentContext trait ────────────────────────────────────────────────────────

/// Pluggable context management for the agent tool-calling loop.
///
/// Implementations control how messages are persisted and how the active
/// context (the window sent to the LLM) is assembled.
#[async_trait::async_trait]
pub trait AgentContext: Send + Sync {
    /// Rebuild the active context from the underlying store.
    ///
    /// Called once at the start of a turn. For persistent stores (LCM),
    /// this restores the conversation history. For in-memory stores,
    /// this is a no-op.
    async fn rebuild(&self, conversation_id: &str) -> AgentJaxResult<()>;

    /// Return the active context as provider-ready API items.
    ///
    /// These items are sent as the conversation history to the LLM provider.
    fn context_items(&self) -> Vec<Value>;

    /// Persist a single message.
    async fn persist_message(&self, msg: &StoredMessage) -> AgentJaxResult<()>;

    /// Persist a batch of messages atomically.
    async fn persist_messages(&self, msgs: &[StoredMessage]) -> AgentJaxResult<()>;
}

// ── LcmAgentContext ───────────────────────────────────────────────────────────

/// Agent context backed by the full LCM engine (SQLite + compaction).
///
/// Used by the main conversation agent. Provides lossless context
/// management with summary DAG and FTS5 search.
pub struct LcmAgentContext {
    engine: Arc<crate::lcm::LcmEngine>,
}

impl LcmAgentContext {
    pub fn new(engine: Arc<crate::lcm::LcmEngine>) -> Self {
        Self { engine }
    }

    pub fn engine(&self) -> &Arc<crate::lcm::LcmEngine> {
        &self.engine
    }
}

#[async_trait::async_trait]
impl AgentContext for LcmAgentContext {
    async fn rebuild(&self, conversation_id: &str) -> AgentJaxResult<()> {
        self.engine.rebuild_active_context(conversation_id)?;
        Ok(())
    }

    fn context_items(&self) -> Vec<Value> {
        self.engine
            .active_context_snapshot()
            .ok()
            .map(|entries| self.engine.context_to_provider_items(&entries))
            .unwrap_or_default()
    }

    async fn persist_message(&self, msg: &StoredMessage) -> AgentJaxResult<()> {
        self.engine.process_message(msg).await?;
        Ok(())
    }

    async fn persist_messages(&self, msgs: &[StoredMessage]) -> AgentJaxResult<()> {
        self.engine.process_messages_batch(msgs).await?;
        Ok(())
    }
}

// ── InMemoryContext ───────────────────────────────────────────────────────────

/// Agent context backed by an in-memory message buffer.
///
/// Used by ephemeral sub-agents. Messages are stored in a `Vec` and
/// never persisted to disk. The context is rebuilt from scratch each
/// turn and discarded when the agent finishes.
pub struct InMemoryContext {
    messages: tokio::sync::Mutex<Vec<StoredMessage>>,
}

impl InMemoryContext {
    pub fn new() -> Self {
        Self {
            messages: tokio::sync::Mutex::new(Vec::new()),
        }
    }
}

impl Default for InMemoryContext {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait::async_trait]
impl AgentContext for InMemoryContext {
    async fn rebuild(&self, _conversation_id: &str) -> AgentJaxResult<()> {
        // In-memory context starts fresh each time — no-op.
        Ok(())
    }

    fn context_items(&self) -> Vec<Value> {
        // Cannot access async Mutex from sync context. Use a best-effort
        // try_lock — if the lock is contended we return empty (rare in practice
        // since context_items is called from the single-threaded agent loop).
        let messages = match self.messages.try_lock() {
            Ok(guard) => guard.clone(),
            Err(_) => return Vec::new(),
        };

        let mut items = Vec::with_capacity(messages.len());
        for msg in &messages {
            items.extend(crate::lcm::stored_message_to_provider_items(msg));
        }
        items
    }

    async fn persist_message(&self, msg: &StoredMessage) -> AgentJaxResult<()> {
        let mut guard = self.messages.lock().await;
        guard.push(msg.clone());
        Ok(())
    }

    async fn persist_messages(&self, msgs: &[StoredMessage]) -> AgentJaxResult<()> {
        let mut guard = self.messages.lock().await;
        guard.extend_from_slice(msgs);
        Ok(())
    }
}

// ── MemoryAgentContext ───────────────────────────────────────────────────────

/// Agent context that reads the parent conversation's recent history from LCM.
///
/// Used by the background memory agent. Provides a lightweight view of the
/// last N messages from the parent conversation — suitable for evaluating
/// whether new memories should be written. Unlike `LcmAgentContext`, this
/// does NOT persist messages (the memory agent doesn't generate conversation
/// history) and does NOT run compaction.
pub struct MemoryAgentContext {
    /// The parent conversation ID to read from.
    parent_conv_id: String,
    /// Maximum number of recent messages to include.
    max_messages: usize,
    /// Cached provider-ready context items, built during `rebuild()`.
    items: tokio::sync::Mutex<Vec<Value>>,
}

impl MemoryAgentContext {
    pub fn new(parent_conv_id: impl Into<String>) -> Self {
        Self {
            parent_conv_id: parent_conv_id.into(),
            max_messages: 20,
            items: tokio::sync::Mutex::new(Vec::new()),
        }
    }

    /// Set the maximum number of recent messages to load.
    pub fn with_max_messages(mut self, n: usize) -> Self {
        self.max_messages = n;
        self
    }
}

#[async_trait::async_trait]
impl AgentContext for MemoryAgentContext {
    async fn rebuild(&self, _conversation_id: &str) -> AgentJaxResult<()> {
        // Open the parent conversation's LCM store and read recent messages.
        let store_path = crate::lcm::lcm_store_path(
            crate::config::constants::DEFAULT_AGENT_ID,
            &self.parent_conv_id,
        )
        .map_err(|e| {
            crate::error::AgentJaxError::internal(format!(
                "Memory agent: failed to get LCM path: {e}"
            ))
        })?;

        let lcm_config = crate::lcm::LcmConfig::default();
        let store = crate::lcm::LcmStore::open(&store_path, lcm_config).map_err(|e| {
            crate::error::AgentJaxError::internal(format!(
                "Memory agent: failed to open LCM store: {e}"
            ))
        })?;

        // Read all messages for the parent conversation and take the last N.
        let all_messages = store
            .get_conversation_messages(&self.parent_conv_id)
            .map_err(|e| {
                crate::error::AgentJaxError::internal(format!(
                    "Memory agent: failed to read LCM messages: {e}"
                ))
            })?;

        let recent: Vec<&StoredMessage> = all_messages
            .iter()
            .rev()
            .take(self.max_messages)
            .rev()
            .collect();

        // Convert to provider-ready items using the canonical implementation.
        let mut items = Vec::with_capacity(recent.len());
        for msg in recent {
            items.extend(crate::lcm::stored_message_to_provider_items(msg));
        }

        let mut guard = self.items.lock().await;
        *guard = items;
        Ok(())
    }

    fn context_items(&self) -> Vec<Value> {
        self.items
            .try_lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    async fn persist_message(&self, _msg: &StoredMessage) -> AgentJaxResult<()> {
        // Memory agent does not generate conversation history — no-op.
        Ok(())
    }

    async fn persist_messages(&self, _msgs: &[StoredMessage]) -> AgentJaxResult<()> {
        // Memory agent does not generate conversation history — no-op.
        Ok(())
    }
}
