//! LCM Engine — top-level coordinator for Lossless Context Management.
//!
//! The `LcmEngine` orchestrates the dual-state memory architecture:
//!
//! 1. **Immutable Store** — Every message is persisted and never modified.
//! 2. **Active Context** — The window sent to the LLM, assembled from
//!    recent raw messages and precomputed summary pointers.
//!
//! ## Context Control Loop (Figure 2)
//!
//! ```text
//! 1. Persist new item into Immutable Store
//! 2. Append item to Active Context (as a pointer)
//! 3. If tokens > τ_soft → trigger async compaction
//! 4. While tokens > τ_hard → block and compact oldest block
//! 5. Return updated Active Context to model
//! ```
//!
//! ## Key Invariants
//!
//! - **Zero-Cost Continuity**: Below τ_soft, the engine acts as a passive
//!   logger — no compaction, no extra latency.
//! - **Atomic Swaps**: Compaction results are atomically swapped into the
//!   active context between LLM turns, never during a response.
//! - **Lossless Retrievability**: Every message can be recovered via
//!   `lcm_expand`, regardless of compaction depth.

use crate::lcm::compaction::{CompactionEngine, Summarizer};
use crate::lcm::dag::SummaryDag;
use crate::lcm::store::LcmStore;
use crate::lcm::types::{
    ContextEntry, FileRefId, LcmConfig, LcmError, LcmId, MessageId,
    StoredMessage, SummaryChild, SummaryId, SummaryKind, estimate_tokens,
};
use crate::lcm::types::MessageRole;
#[cfg(test)]
use crate::lcm::compaction::NoopSummarizer;
use std::collections::HashSet;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

// ── LcmEngine ───────────────────────────────────────────────────────────────

/// The top-level LCM engine.
///
/// Coordinates the immutable store, summary DAG, compaction engine,
/// file handler, and active context assembly.
pub struct LcmEngine {
    /// The SQLite-backed immutable store.
    store: Arc<LcmStore>,
    /// The summary DAG manager.
    dag: SummaryDag,
    /// The three-level compaction engine.
    compaction: CompactionEngine,
    /// LCM configuration.
    config: LcmConfig,
    /// Background compaction signal sender.
    compaction_tx: mpsc::UnboundedSender<()>,
    /// Background compaction signal receiver (taken by spawn_compaction_task).
    compaction_rx: Mutex<Option<mpsc::UnboundedReceiver<()>>>,
    /// The current active context — the window sent to the LLM.
    active_context: Mutex<ActiveContextState>,
}

/// Internal mutable state for the active context.
struct ActiveContextState {
    /// The ordered list of context entries.
    entries: Vec<ContextEntry>,
    /// Estimated total token count of all entries.
    token_count: u32,
    /// The set of message IDs currently in the active context (raw form).
    /// Used to avoid re-processing messages that are already present.
    active_message_ids: HashSet<String>,
    /// Background compaction handle, if one is running.
    compaction_running: bool,
}

impl ActiveContextState {
    fn new() -> Self {
        Self {
            entries: Vec::new(),
            token_count: 0,
            active_message_ids: HashSet::new(),
            compaction_running: false,
        }
    }
}

impl LcmEngine {
    // ── Construction ──────────────────────────────────────────────────

    /// Create a new LCM engine backed by the given store.
    ///
    /// The `summarizer` is used for Level 1 and Level 2 compaction.
    /// Pass `Arc::new(NoopSummarizer)` if you want to force Level 3
    /// truncation only (useful for testing or when no LLM is available).
    pub fn new(
        store: Arc<LcmStore>,
        summarizer: Arc<dyn Summarizer>,
        config: LcmConfig,
    ) -> Self {
        let dag = SummaryDag::new(store.clone());
        let truncation_max_tokens = config.truncation_max_tokens;

        // Build a token counter that uses the real tokenizer when available.
        let tokenizer_model_id = config.tokenizer_model_id.clone();
        let count_tokens: crate::lcm::compaction::TokenCounter = Arc::new(move |text: &str| {
            if let Some(ref model_id) = tokenizer_model_id {
                match crate::conversation_store::count_text_tokens(model_id, text) {
                    Ok(count) => return count as u32,
                    Err(_) => { /* fall through to heuristic */ }
                }
            }
            crate::lcm::types::estimate_tokens(text)
        });

        let compaction = CompactionEngine::new(summarizer, truncation_max_tokens, count_tokens);

        let (compaction_tx, compaction_rx) = mpsc::unbounded_channel();

        Self {
            store,
            dag,
            compaction,
            config,
            compaction_tx,
            compaction_rx: Mutex::new(Some(compaction_rx)),
            active_context: Mutex::new(ActiveContextState::new()),
        }
    }

    /// Create a new LCM engine with the NoopSummarizer (Level 3 only).
    #[cfg(test)]
    pub fn new_for_testing(store: Arc<LcmStore>, config: LcmConfig) -> Self {
        Self::new(store, Arc::new(NoopSummarizer), config)
    }

    /// Spawn the background compaction task for this engine.
    ///
    /// This should be called once right after construction. It spawns a
    /// tokio task that listens on the compaction channel and runs
    /// `compact_oldest_block` when signalled.
    ///
    /// The task holds a `Weak` reference to the engine, so it will
    /// automatically terminate when the engine is dropped.
    pub fn spawn_compaction_task(self: &Arc<Self>) {
        let engine_weak = Arc::downgrade(self);
        let timeout_secs = self.config.compaction_timeout_secs;

        // Take the receiver out of the mutex.
        let rx_opt = match self.compaction_rx.lock() {
            Ok(mut guard) => guard.take(),
            Err(_) => {
                log::error!("LCM: Failed to acquire compaction_rx lock for spawning task");
                return;
            }
        };

        let Some(mut rx) = rx_opt else {
            log::error!("LCM: Compaction receiver already taken (spawn_compaction_task called twice?)");
            return;
        };

        tokio::spawn(async move {
            log::info!(
                "LCM background compaction task started (timeout: {}s)",
                timeout_secs
            );

            loop {
                match rx.recv().await {
                    Some(()) => {
                        // Upgrade weak ref — if engine is dropped, terminate.
                        let Some(engine) = engine_weak.upgrade() else {
                            log::debug!("LCM compaction task: engine dropped, exiting");
                            break;
                        };

                        log::debug!("LCM async compaction: received signal, compacting...");

                        // Run compaction with timeout.
                        let result = tokio::time::timeout(
                            std::time::Duration::from_secs(timeout_secs as u64),
                            engine.compact_oldest_block(),
                        )
                        .await;

                        match result {
                            Ok(Ok(_)) => {
                                log::debug!("LCM async compaction: completed successfully");
                            }
                            Ok(Err(e)) => {
                                log::warn!("LCM async compaction failed: {e}");
                            }
                            Err(_elapsed) => {
                                log::warn!(
                                    "LCM async compaction timed out after {}s",
                                    timeout_secs
                                );
                            }
                        }

                        // Clear the running flag so next trigger can fire.
                        if let Ok(mut ctx) = engine.active_context.lock() {
                            ctx.compaction_running = false;
                        }
                    }
                    None => {
                        // Channel closed — engine was dropped.
                        log::debug!("LCM compaction task: channel closed, exiting");
                        break;
                    }
                }
            }

            log::debug!("LCM background compaction task exited");
        });

        log::info!("LCM background compaction task spawned");
    }

    /// Returns a reference to the underlying store.
    pub fn store(&self) -> &Arc<LcmStore> {
        &self.store
    }

    /// Returns the current configuration.
    #[allow(dead_code)]
    pub fn config(&self) -> &LcmConfig {
        &self.config
    }

    // ── Context Control Loop ──────────────────────────────────────────

    /// Process a new message through the LCM control loop.
    ///
    /// This implements the core algorithm from Figure 2 of the paper:
    ///
    /// 1. Persist the message in the immutable store.
    /// 2. Append it to the active context.
    /// 3. If tokens exceed τ_soft, trigger async compaction.
    /// 4. If tokens exceed τ_hard, block until sufficient space is freed.
    ///
    /// Returns the updated active context entries.
    pub async fn process_message(
        &self,
        msg: &StoredMessage,
    ) -> Result<Vec<ContextEntry>, LcmError> {
        // Step 1: Persist to immutable store.
        self.store.persist_message(msg)?;

        // Step 2: Append to active context.
        {
            let mut ctx = self.active_context.lock().map_err(|e| {
                LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
            })?;

            // Deduplicate by message ID.
            if !ctx.active_message_ids.insert(msg.id.to_string()) {
                // Already in context — skip.
                return Ok(ctx.entries.clone());
            }

            let entry = ContextEntry::RawMessage {
                id: msg.id.clone(),
                role: msg.role,
                content: msg.content.clone(),
                metadata: msg.metadata.clone(),
            };

            ctx.token_count += msg.token_count;
            ctx.entries.push(entry);
        }

        // Step 3: Check soft threshold → async compaction.
        let should_compact_async = {
            let ctx = self.active_context.lock().map_err(|e| {
                LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
            })?;
            ctx.token_count > self.config.soft_token_threshold && !ctx.compaction_running
        };

        if should_compact_async {
            self.trigger_async_compaction();
        }

        // Step 4: Check hard threshold → blocking compaction.
        self.ensure_below_hard_threshold().await?;

        // Return current active context snapshot.
        let ctx = self.active_context.lock().map_err(|e| {
            LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
        })?;
        Ok(ctx.entries.clone())
    }

    /// Process multiple messages in a single batch.
    ///
    /// Persists all messages in one SQLite transaction, then appends them
    /// all to the active context, and runs a single threshold check at the end.
    /// This is significantly more efficient than calling `process_message`
    /// individually for each message.
    pub async fn process_messages_batch(
        &self,
        messages: &[StoredMessage],
    ) -> Result<Vec<ContextEntry>, LcmError> {
        if messages.is_empty() {
            let entries = {
                let ctx = self.active_context.lock().map_err(|e| {
                    LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
                })?;
                ctx.entries.clone()
            };
            return Ok(entries);
        }

        // Step 1: Persist all messages in a single transaction.
        self.store.persist_messages(messages)?;

        // Step 2: Append to active context (guard released before any await).
        let (should_compact_async, token_count_exceeded) = {
            let mut ctx = self.active_context.lock().map_err(|e| {
                LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
            })?;

            for msg in messages {
                if ctx.active_message_ids.insert(msg.id.to_string()) {
                    let entry = ContextEntry::RawMessage {
                        id: msg.id.clone(),
                        role: msg.role,
                        content: msg.content.clone(),
                        metadata: msg.metadata.clone(),
                    };
                    ctx.token_count += msg.token_count;
                    ctx.entries.push(entry);
                }
            }

            let soft = ctx.token_count > self.config.soft_token_threshold && !ctx.compaction_running;
            let hard = ctx.token_count > self.config.hard_token_threshold;
            (soft, hard)
        }; // MutexGuard dropped here

        // Step 3: Trigger async compaction if above soft threshold.
        if should_compact_async {
            self.trigger_async_compaction();
        }

        // Step 4: Blocking compaction if above hard threshold (await-safe: no guard held).
        if token_count_exceeded {
            self.ensure_below_hard_threshold().await?;
        }

        // Return final active context snapshot.
        let ctx = self.active_context.lock().map_err(|e| {
            LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
        })?;
        Ok(ctx.entries.clone())
    }

    /// Get the current active context snapshot without modifying anything.
    pub fn active_context_snapshot(&self) -> Result<Vec<ContextEntry>, LcmError> {
        let ctx = self.active_context.lock().map_err(|e| {
            LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
        })?;
        Ok(ctx.entries.clone())
    }

    /// Get the current estimated token count.
    /// Currently only used in tests.
    #[allow(dead_code)]
    pub fn token_count(&self) -> Result<u32, LcmError> {
        let ctx = self.active_context.lock().map_err(|e| {
            LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
        })?;
        Ok(ctx.token_count)
    }

    /// Count tokens for a text string using the real tokenizer when available,
    /// falling back to the 4:1 character heuristic.
    ///
    /// This bridges the LCM layer with the globally-cached HuggingFace tokenizer
    /// managed by `conversation_store::count_text_tokens`.
    #[allow(dead_code)]
    pub fn count_tokens(&self, text: &str) -> u32 {
        if let Some(ref model_id) = self.config.tokenizer_model_id {
            match crate::conversation_store::count_text_tokens(model_id, text) {
                Ok(count) => count as u32,
                Err(_) => {
                    // Fall back to heuristic on tokenizer error.
                    crate::lcm::types::estimate_tokens(text)
                }
            }
        } else {
            crate::lcm::types::estimate_tokens(text)
        }
    }

    /// Check if the active context is currently above the hard threshold.
    #[allow(dead_code)]
    pub fn is_above_hard_threshold(&self) -> Result<bool, LcmError> {
        let ctx = self.active_context.lock().map_err(|e| {
            LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
        })?;
        Ok(ctx.token_count > self.config.hard_token_threshold)
    }

    // ── Compaction ────────────────────────────────────────────────────

    /// Trigger asynchronous compaction by sending a signal via the channel.
    ///
    /// The background compaction task (spawned via `spawn_compaction_task`)
    /// receives the signal and runs `compact_oldest_block` in a background
    /// tokio task with a configurable timeout.
    fn trigger_async_compaction(&self) {
        let mut ctx = match self.active_context.lock() {
            Ok(c) => c,
            Err(_) => return,
        };
        if ctx.compaction_running {
            return;
        }
        ctx.compaction_running = true;
        drop(ctx);

        log::info!(
            "LCM async compaction triggered (threshold: {} tokens)",
            self.config.soft_token_threshold
        );

        // Send a compaction signal via the channel.
        if let Err(e) = self.compaction_tx.send(()) {
            log::warn!("LCM async compaction signal failed: {e}");
            // Reset the flag so compaction can be retried.
            if let Ok(mut ctx) = self.active_context.lock() {
                ctx.compaction_running = false;
            }
        }
    }

    /// Ensure the active context is below the hard threshold.
    ///
    /// Blocks (via async compaction) until enough space is freed.
    async fn ensure_below_hard_threshold(&self) -> Result<(), LcmError> {
        loop {
            let above_threshold = {
                let ctx = self.active_context.lock().map_err(|e| {
                    LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
                })?;
                ctx.token_count > self.config.hard_token_threshold
            };

            if !above_threshold {
                return Ok(());
            }

            // Block and compact.
            self.compact_oldest_block().await?;
        }
    }

    /// Compact the oldest block of messages in the active context.
    ///
    /// 1. Identifies the oldest contiguous block of raw messages.
    /// 2. Runs them through the Three-Level Escalation protocol.
    /// 3. Creates a summary node in the DAG.
    /// 4. Atomically replaces the messages with a SummaryPointer.
    pub async fn compact_oldest_block(&self) -> Result<(), LcmError> {
        // Take a snapshot of the oldest messages to compact.
        let block = {
            let ctx = self.active_context.lock().map_err(|e| {
                LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
            })?;

            let block_size = self.config.max_compact_block_size;
            let oldest_messages: Vec<ContextEntry> = ctx
                .entries
                .iter()
                .filter(|e| matches!(e, ContextEntry::RawMessage { .. }))
                .take(block_size)
                .cloned()
                .collect();

            if oldest_messages.is_empty() {
                // If there are no raw messages to compact, try compacting
                // the oldest summary pointers into a condensed summary.
                let oldest_summaries: Vec<ContextEntry> = ctx
                    .entries
                    .iter()
                    .filter(|e| matches!(e, ContextEntry::SummaryPointer { .. }))
                    .take(block_size / 2) // Fewer summaries needed.
                    .cloned()
                    .collect();

                if oldest_summaries.is_empty() {
                    return Ok(()); // Nothing to compact.
                }
                oldest_summaries
            } else {
                oldest_messages
            }
        };

        // Resolve the block to StoredMessages and/or SummaryIds.
        let (messages, summary_ids) = self.resolve_block(&block).await?;
        let conversation_id = self.infer_conversation_id(&messages, &summary_ids)?;

        let now_ms = crate::conversation_store_utils::now_unix_ms();

        if !messages.is_empty() {
            // Leaf compaction: messages → summary.
            let input_tokens: u32 = messages.iter().map(|m| m.token_count).sum();
            let target_tokens = (input_tokens / 3).max(256); // Aim for ~1/3 compression.

            let (summary_text, compaction_level) = self
                .compaction
                .escalate_summarize(&messages, target_tokens)
                .await?;

            // Collect file refs from messages being compacted for propagation.
            let propagated_file_refs: Vec<FileRefId> = messages
                .iter()
                .flat_map(|m| m.file_refs.iter().cloned())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();

            let summary_id = SummaryId::new();
            let summary_node = CompactionEngine::build_summary_node_with_refs(
                summary_id.clone(),
                &conversation_id,
                &summary_text,
                compaction_level,
                SummaryKind::Leaf,
                now_ms,
                self.compaction.token_counter(),
                propagated_file_refs.clone(),
            );

            let message_ids: Vec<MessageId> = messages.iter().map(|m| m.id.clone()).collect();

            self.dag
                .create_leaf_summary(&summary_node, &message_ids)?;

            // Atomically replace messages with summary pointer in active context.
            self.replace_in_active_context(
                &block,
                ContextEntry::SummaryPointer {
                    summary_id: summary_node.id.clone(),
                    text: summary_text,
                    child_ids: message_ids.into_iter().map(|id| LcmId::from(id.as_str())).collect(),
                    file_refs: propagated_file_refs,
                },
            )?;

            log::info!(
                "LCM compacted {} messages ({} tokens → {} tokens, level {})",
                messages.len(),
                input_tokens,
                estimate_tokens(&summary_node.text),
                compaction_level,
            );
        } else if !summary_ids.is_empty() {
            // ── Condensed compaction: multiple summaries → one condensed summary ──
            // Read the summary texts from the store and create temporary messages
            // so we can use the existing Three-Level Escalation protocol.
            let mut summary_messages = Vec::new();
            let mut propagated_file_refs: std::collections::HashSet<FileRefId> =
                std::collections::HashSet::new();
            for sid in &summary_ids {
                if let Some(summary) = self.store.get_summary(sid)? {
                    // Carry over file refs from child summaries.
                    for fr in &summary.file_refs {
                        propagated_file_refs.insert(fr.clone());
                    }
                    let tmp_msg = StoredMessage {
                        id: LcmId::new(),
                        conversation_id: conversation_id.clone(),
                        role: MessageRole::Assistant,
                        content: summary.text.clone(),
                        token_count: summary.token_count,
                        timestamp_unix_ms: summary.created_at_unix_ms,
                        covered_by: None,
                        metadata: std::collections::BTreeMap::new(),
                        file_refs: summary.file_refs.clone(),
                    };
                    summary_messages.push(tmp_msg);
                }
            }

            if summary_messages.len() < 2 {
                // Need at least 2 summaries to condense.
                // Reset compaction_running so we can try again later.
                if let Ok(mut ctx) = self.active_context.lock() {
                    ctx.compaction_running = false;
                }
                return Ok(());
            }

            let input_tokens: u32 = summary_messages.iter().map(|m| m.token_count).sum();
            let target_tokens = (input_tokens / 3).max(256); // Aim for ~1/3 compression.

            let (condensed_text, compaction_level) = self
                .compaction
                .escalate_summarize(&summary_messages, target_tokens)
                .await?;

            let propagated_refs: Vec<FileRefId> = propagated_file_refs.into_iter().collect();

            let condensed_id = SummaryId::new();
            let condensed_node = CompactionEngine::build_summary_node_with_refs(
                condensed_id.clone(),
                &conversation_id,
                &condensed_text,
                compaction_level,
                SummaryKind::Condensed,
                now_ms,
                self.compaction.token_counter(),
                propagated_refs.clone(),
            );

            self.dag
                .create_condensed_summary(&condensed_node, &summary_ids)?;

            // Collect all child IDs from the original summaries for the pointer.
            let all_child_ids: Vec<LcmId> = summary_ids
                .iter()
                .map(|sid| LcmId::from(sid.as_str()))
                .collect();

            // Atomically replace summaries with condensed pointer.
            self.replace_in_active_context(
                &block,
                ContextEntry::SummaryPointer {
                    summary_id: condensed_node.id.clone(),
                    text: condensed_text,
                    child_ids: all_child_ids,
                    file_refs: propagated_refs,
                },
            )?;

            log::info!(
                "LCM condensed {} summaries ({} tokens → {} tokens, level {})",
                summary_ids.len(),
                input_tokens,
                estimate_tokens(&condensed_node.text),
                compaction_level,
            );
        }

        Ok(())
    }

    /// Resolve a block of context entries into stored messages and summary IDs.
    async fn resolve_block(
        &self,
        block: &[ContextEntry],
    ) -> Result<(Vec<StoredMessage>, Vec<SummaryId>), LcmError> {
        let mut messages = Vec::new();
        let mut summary_ids = Vec::new();

        for entry in block {
            match entry {
                ContextEntry::RawMessage { id, .. } => {
                    if let Some(msg) = self.store.get_message(id)? {
                        messages.push(msg);
                    }
                }
                ContextEntry::SummaryPointer { summary_id, .. } => {
                    summary_ids.push(summary_id.clone());
                }
                ContextEntry::FilePointer { .. } => {
                    // File pointers are never compacted.
                }
            }
        }

        Ok((messages, summary_ids))
    }

    /// Infer the conversation ID from the data available.
    fn infer_conversation_id(
        &self,
        messages: &[StoredMessage],
        summary_ids: &[SummaryId],
    ) -> Result<String, LcmError> {
        if let Some(msg) = messages.first() {
            return Ok(msg.conversation_id.clone());
        }
        if let Some(sid) = summary_ids.first() {
            if let Some(summary) = self.store.get_summary(sid)? {
                return Ok(summary.conversation_id);
            }
        }
        Err(LcmError::Compaction(
            "Cannot infer conversation ID for compaction".to_string(),
        ))
    }

    /// Atomically replace entries in the active context.
    fn replace_in_active_context(
        &self,
        old_entries: &[ContextEntry],
        new_entry: ContextEntry,
    ) -> Result<(), LcmError> {
        let mut ctx = self.active_context.lock().map_err(|e| {
            LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
        })?;

        // Collect IDs of messages being removed from active context.
        let removed_ids: HashSet<String> = old_entries
            .iter()
            .filter_map(|e| match e {
                ContextEntry::RawMessage { id, .. } => Some(id.to_string()),
                ContextEntry::SummaryPointer { summary_id, .. } => Some(summary_id.to_string()),
                _ => None,
            })
            .collect();

        // Calculate token reduction.
        let old_tokens: u32 = old_entries
            .iter()
            .map(|e| match e {
                ContextEntry::RawMessage { content, .. } => estimate_tokens(content),
                ContextEntry::SummaryPointer { text, .. } => estimate_tokens(text),
                ContextEntry::FilePointer {
                    exploration_summary, ..
                } => estimate_tokens(exploration_summary),
            })
            .sum();

        let new_tokens = match &new_entry {
            ContextEntry::RawMessage { content, .. } => estimate_tokens(content),
            ContextEntry::SummaryPointer { text, .. } => estimate_tokens(text),
            ContextEntry::FilePointer {
                exploration_summary, ..
            } => estimate_tokens(exploration_summary),
        };

        // Find and replace the old entries with the new one.
        // We do this by rebuilding the entries list, replacing the first
        // contiguous block of old entries with the new entry.
        let mut new_entries = Vec::new();
        let mut replaced = false;

        // Use a simple sliding window approach to find and replace.
        let old_ids: HashSet<String> = old_entries
            .iter()
            .filter_map(|e| match e {
                ContextEntry::RawMessage { id, .. } => Some(id.to_string()),
                ContextEntry::SummaryPointer { summary_id, .. } => Some(summary_id.to_string()),
                _ => None,
            })
            .collect();

        let mut skip_until_done = false;
        for entry in &ctx.entries {
            let entry_id = match entry {
                ContextEntry::RawMessage { id, .. } => Some(id.to_string()),
                ContextEntry::SummaryPointer { summary_id, .. } => Some(summary_id.to_string()),
                _ => None,
            };

            let is_in_old_block = entry_id
                .as_ref()
                .map_or(false, |id| old_ids.contains(id));

            if !replaced && is_in_old_block {
                if !skip_until_done {
                    new_entries.push(new_entry.clone());
                    replaced = true;
                }
                skip_until_done = true;
            } else if skip_until_done && is_in_old_block {
                // Still in the old block — skip.
            } else {
                skip_until_done = false;
                new_entries.push(entry.clone());
            }
        }

        // If we didn't find the entries (they may have been rearranged),
        // just append the new entry and remove the old ones individually.
        if !replaced {
            new_entries = ctx
                .entries
                .iter()
                .filter(|e| {
                    let id = match e {
                        ContextEntry::RawMessage { id, .. } => Some(id.to_string()),
                        ContextEntry::SummaryPointer { summary_id, .. } => {
                            Some(summary_id.to_string())
                        }
                        _ => None,
                    };
                    !id.map_or(false, |i| old_ids.contains(&i))
                })
                .cloned()
                .collect();
            new_entries.push(new_entry.clone());
        }

        // Update IDs set.
        for removed_id in &removed_ids {
            ctx.active_message_ids.remove(removed_id);
        }

        ctx.entries = new_entries;
        ctx.token_count = ctx.token_count.saturating_sub(old_tokens) + new_tokens;
        ctx.compaction_running = false;

        Ok(())
    }

    // Note: file_refs propagation to StoredMessage happens in runtime/engine.rs
    // when the tool result message is persisted — the caller sets
    // StoredMessage.file_refs before calling process_message.

    // ── Rebuild from Store ────────────────────────────────────────────

    /// Rebuild the active context from the immutable store.
    ///
    /// Used when loading a conversation that has already been partially
    /// compacted. Messages covered by summaries are replaced with their
    /// summary pointers in the active context.
    pub fn rebuild_active_context(
        &self,
        conversation_id: &str,
    ) -> Result<Vec<ContextEntry>, LcmError> {
        let messages = self.store.get_conversation_messages(conversation_id)?;

        let mut entries = Vec::new();
        let mut seen_summaries = HashSet::new();
        let mut token_count: u32 = 0;
        let mut active_ids = HashSet::new();

        for msg in messages {
            match &msg.covered_by {
                Some(summary_id) => {
                    // This message is covered by a summary — show the summary
                    // pointer instead of the raw message.
                    if seen_summaries.insert(summary_id.to_string()) {
                        if let Some(summary) = self.store.get_summary(summary_id)? {
                            let children = self.store.get_summary_children(summary_id)?;
                            let child_ids: Vec<LcmId> = children
                                .iter()
                                .flat_map(|c| match c {
                                    SummaryChild::Messages { ids } => {
                                        ids.iter().map(|id| LcmId::from(id.as_str()))
                                            .collect::<Vec<_>>()
                                    }
                                    SummaryChild::Summaries { ids } => {
                                        ids.iter().map(|id| LcmId::from(id.as_str()))
                                            .collect::<Vec<_>>()
                                    }
                                })
                                .collect();

                            let entry = ContextEntry::SummaryPointer {
                                summary_id: summary_id.clone(),
                                text: summary.text.clone(),
                                child_ids,
                                file_refs: summary.file_refs.clone(),
                            };

                            token_count += estimate_tokens(&summary.text);
                            entries.push(entry);
                            active_ids.insert(summary_id.to_string());
                        }
                    }
                }
                None => {
                    // Raw message — include directly.
                    let entry = ContextEntry::RawMessage {
                        id: msg.id.clone(),
                        role: msg.role,
                        content: msg.content.clone(),
                        metadata: msg.metadata.clone(),
                    };

                    token_count += msg.token_count;
                    entries.push(entry);
                    active_ids.insert(msg.id.to_string());
                }
            }
        }

        // Update internal state.
        {
            let mut ctx = self.active_context.lock().map_err(|e| {
                LcmError::Concurrency(format!("Failed to acquire context lock: {e}"))
            })?;
            ctx.entries = entries;
            ctx.token_count = token_count;
            ctx.active_message_ids = active_ids;
        }

        self.active_context_snapshot()
    }

    // ── Context → Provider Items ──────────────────────────────────────

    /// Convert LCM context entries to provider API input items.
    ///
    /// This bridges the LCM active context (ContextEntry) with the
    /// provider API format (Vec<Value>). Each entry becomes a message
    /// in the provider's expected format.
    pub fn context_to_provider_items(&self, entries: &[ContextEntry]) -> Vec<serde_json::Value> {
        let mut items = Vec::with_capacity(entries.len());

        for entry in entries {
            match entry {
                ContextEntry::RawMessage { role, content, metadata, .. } => {
                    // ── Structured tool messages (via metadata) ────────────
                    // When a message carries a `message_type` in its metadata,
                    // reconstruct the proper Responses-API item shape so that
                    // function_call / function_call_output pairs are correctly
                    // linked by call_id. This preserves fidelity that would
                    // otherwise be lost when the LCM compresses history.
                    if let Some(msg_type) = metadata.get("message_type").and_then(|v| v.as_str()) {
                        match msg_type {
                            "function_call" => {
                                let call_id = metadata
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let name = metadata
                                    .get("tool_name")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                let arguments = metadata
                                    .get("arguments")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("{}");
                                items.push(serde_json::json!({
                                    "type": "function_call",
                                    "call_id": call_id,
                                    "name": name,
                                    "arguments": arguments,
                                }));
                                continue;
                            }
                            "function_call_output" => {
                                let call_id = metadata
                                    .get("call_id")
                                    .and_then(|v| v.as_str())
                                    .unwrap_or("");
                                items.push(serde_json::json!({
                                    "type": "function_call_output",
                                    "call_id": call_id,
                                    "output": content,
                                }));
                                continue;
                            }
                            _ => {
                                // Unknown message_type — fall through to
                                // role-based fallback below.
                            }
                        }
                    }

                    // ── Role-based fallback (legacy messages without metadata) ──
                    let provider_role = match role {
                        crate::lcm::types::MessageRole::User => "user",
                        crate::lcm::types::MessageRole::Assistant => "assistant",
                        // Legacy Tool messages without metadata: map to "user"
                        // since "tool" is not a valid role in the Responses API.
                        crate::lcm::types::MessageRole::Tool => "user",
                    };
                    let text_type = match role {
                        crate::lcm::types::MessageRole::Assistant => "output_text",
                        _ => "input_text",
                    };
                    let mut item = serde_json::json!({
                        "role": provider_role,
                        "content": [{
                            "type": text_type,
                            "text": content
                        }]
                    });
                    // Preserve phase (commentary / final_answer) if present in metadata.
                    if let Some(phase) = metadata.get("phase").and_then(|v| v.as_str()) {
                        item["phase"] = serde_json::Value::String(phase.to_string());
                    }
                    items.push(item);
                }
                ContextEntry::SummaryPointer { text, child_ids, .. } => {
                    // Render summary as a developer/assistant note with
                    // expansion hints so the model knows it can drill down.
                    let child_count = child_ids.len();
                    let summary_text = if child_count > 0 {
                        format!(
                            "[LCM Summary — covers {child_count} messages. \
                             Use lcm_expand to recover details.]\n\n{text}"
                        )
                    } else {
                        format!("[LCM Summary]\n\n{text}")
                    };
                    items.push(serde_json::json!({
                        "role": "assistant",
                        "content": [{
                            "type": "output_text",
                            "text": summary_text
                        }]
                    }));
                }
                ContextEntry::FilePointer {
                    path,
                    exploration_summary,
                    ..
                } => {
                    items.push(serde_json::json!({
                        "role": "user",
                        "content": [{
                            "type": "input_text",
                            "text": format!(
                                "[File Reference: {path}]\n{exploration_summary}"
                            )
                        }]
                    }));
                }
            }
        }

        items
    }



}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lcm::store::LcmStore;
    use crate::lcm::types::{FileRefId, LcmConfig};
    use std::collections::BTreeMap;

    fn make_engine() -> LcmEngine {
        let config = LcmConfig {
            soft_token_threshold: 100,
            hard_token_threshold: 500,
            ..LcmConfig::default()
        };
        let store = Arc::new(LcmStore::open_in_memory(config.clone()).unwrap());
        LcmEngine::new_for_testing(store, config)
    }

    fn make_msg(id: &str, content: &str, role: MessageRole) -> StoredMessage {
        StoredMessage::new(
            MessageId::from(id),
            "test-conv",
            role,
            content,
            estimate_tokens(content),
            1000 + id.len() as i64,
        )
    }

    #[tokio::test]
    async fn test_process_message_adds_to_context() {
        let engine = make_engine();
        let msg = make_msg("msg-1", "Hello, world!", MessageRole::User);

        let entries = engine.process_message(&msg).await.unwrap();
        assert_eq!(entries.len(), 1);

        if let ContextEntry::RawMessage { content, .. } = &entries[0] {
            assert_eq!(content, "Hello, world!");
        } else {
            panic!("Expected RawMessage");
        }
    }

    #[tokio::test]
    async fn test_process_message_deduplicates() {
        let engine = make_engine();
        let msg = make_msg("msg-1", "Hello!", MessageRole::User);

        engine.process_message(&msg).await.unwrap();
        engine.process_message(&msg).await.unwrap(); // Duplicate.

        let entries = engine.active_context_snapshot().unwrap();
        assert_eq!(entries.len(), 1); // Still only one entry.
    }

    #[tokio::test]
    async fn test_token_count_tracking() {
        let engine = make_engine();
        let msg = make_msg("msg-1", "Hello, world! This is a test message.", MessageRole::User);

        engine.process_message(&msg).await.unwrap();
        let count = engine.token_count().unwrap();
        assert!(count > 0);
    }

    #[tokio::test]
    async fn test_rebuild_active_context() {
        let engine = make_engine();

        // Insert messages directly into store (bypassing engine).
        let msg1 = make_msg("msg-a", "First message", MessageRole::User);
        let msg2 = make_msg("msg-b", "Second message", MessageRole::Assistant);

        engine.store.persist_message(&msg1).unwrap();
        engine.store.persist_message(&msg2).unwrap();

        let entries = engine
            .rebuild_active_context("test-conv")
            .unwrap();
        assert_eq!(entries.len(), 2);
    }

    #[tokio::test]
    async fn test_below_soft_threshold_no_compaction() {
        let config = LcmConfig {
            soft_token_threshold: 10000, // Very high.
            hard_token_threshold: 20000,
            ..Default::default()
        };
        let store = Arc::new(LcmStore::open_in_memory(config.clone()).unwrap());
        let engine = LcmEngine::new_for_testing(store, config);

        let msg = make_msg("msg-1", "Short message", MessageRole::User);
        let entries = engine.process_message(&msg).await.unwrap();

        // Should be raw message, not a summary pointer.
        assert!(matches!(entries[0], ContextEntry::RawMessage { .. }));
    }

    // ── Context → Provider Items ──────────────────────────────────────

    #[test]
    fn test_context_to_provider_items_raw_message() {
        let engine = make_engine();
        let entries = vec![ContextEntry::RawMessage {
            id: MessageId::from("msg-1"),
            role: MessageRole::User,
            content: "Hello".to_string(),
            metadata: BTreeMap::new(),
        }];

        let items = engine.context_to_provider_items(&entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
        assert_eq!(items[0]["content"][0]["text"], "Hello");
    }

    #[test]
    fn test_context_to_provider_items_summary_pointer() {
        let engine = make_engine();
        let entries = vec![ContextEntry::SummaryPointer {
            summary_id: SummaryId::from("sum-1"),
            text: "Key discussion points".to_string(),
            child_ids: vec![LcmId::from("msg-1"), LcmId::from("msg-2")],
            file_refs: Vec::new(),
        }];

        let items = engine.context_to_provider_items(&entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "assistant");
        let text = items[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("covers 2 messages"));
        assert!(text.contains("Key discussion points"));
    }

    #[test]
    fn test_context_to_provider_items_file_pointer() {
        let engine = make_engine();
        let entries = vec![ContextEntry::FilePointer {
            file_id: FileRefId::from("file-1"),
            path: "/tmp/data.csv".to_string(),
            exploration_summary: "CSV with 100 rows, 5 columns".to_string(),
        }];

        let items = engine.context_to_provider_items(&entries);
        assert_eq!(items.len(), 1);
        assert_eq!(items[0]["role"], "user");
        let text = items[0]["content"][0]["text"].as_str().unwrap();
        assert!(text.contains("/tmp/data.csv"));
        assert!(text.contains("CSV with 100 rows"));
    }

    #[test]
    fn test_context_to_provider_items_mixed() {
        let engine = make_engine();
        let entries = vec![
            ContextEntry::RawMessage {
                id: MessageId::from("msg-1"),
                role: MessageRole::User,
                content: "First".to_string(),
                metadata: BTreeMap::new(),
            },
            ContextEntry::SummaryPointer {
                summary_id: SummaryId::from("sum-1"),
                text: "Summary".to_string(),
                child_ids: vec![],
                file_refs: vec![],
            },
            ContextEntry::FilePointer {
                file_id: FileRefId::from("file-1"),
                path: "data.json".to_string(),
                exploration_summary: "JSON data".to_string(),
            },
        ];

        let items = engine.context_to_provider_items(&entries);
        assert_eq!(items.len(), 3);
    }

}
