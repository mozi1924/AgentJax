//! Core data types for the LCM (Lossless Context Management) system.
//!
//! These types define the dual-state memory architecture:
//! - **Immutable Store**: Persisted, never-modified message history
//! - **Active Context**: The window actually sent to the LLM
//! - **Summary DAG**: Hierarchical compressed representations with lossless pointers

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

// ── Identifiers ────────────────────────────────────────────────────────────

/// A unique identifier within the LCM system. Uses UUID v7 for time-ordered
/// generation when created by the engine; may also be a content hash for
/// deterministic derivation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[serde(transparent)]
pub struct LcmId(pub String);

impl LcmId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for LcmId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl From<String> for LcmId {
    fn from(s: String) -> Self {
        Self(s)
    }
}

impl From<&str> for LcmId {
    fn from(s: &str) -> Self {
        Self(s.to_string())
    }
}

/// Identifier for a summary node in the DAG.
pub type SummaryId = LcmId;

/// Identifier for a large-file reference.
pub type FileRefId = LcmId;

/// Identifier for a message in the immutable store.
pub type MessageId = LcmId;

// ── Message Storage ─────────────────────────────────────────────────────────

/// The role of a message in the conversation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum MessageRole {
    User,
    Assistant,
    Tool,
}

impl MessageRole {
    pub fn as_str(&self) -> &'static str {
        match self {
            MessageRole::User => "user",
            MessageRole::Assistant => "assistant",
            MessageRole::Tool => "tool",
        }
    }
}

impl std::fmt::Display for MessageRole {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// A single message in the immutable store.
///
/// Messages are **never modified** after insertion. When compaction occurs,
/// the `covered_by` field is updated to point to the summary node that now
/// represents this message in the active context, but the original `content`
/// is preserved forever.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredMessage {
    /// Unique identifier for this message.
    pub id: MessageId,

    /// The conversation this message belongs to.
    pub conversation_id: String,

    /// The role of the speaker.
    pub role: MessageRole,

    /// The full, original message content — never truncated.
    pub content: String,

    /// Estimated token count of the content.
    pub token_count: u32,

    /// Unix timestamp in milliseconds when the message was created.
    pub timestamp_unix_ms: i64,

    /// If this message has been compacted, the ID of the summary node that
    /// covers it. `None` means the message is still in the active context
    /// in its raw form.
    pub covered_by: Option<SummaryId>,

    /// Reasoning / thinking content (chain-of-thought) for this message.
    /// Stored directly alongside the message so loading is a single query.
    /// Only populated for assistant messages with CoC reasoning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thinking: Option<String>,

    /// Global sequence number within the conversation.
    /// Provides monotonic ordering for context reconstruction.
    /// 1-based — user messages get seq=1, then incrementing for each item.
    pub seq: u32,

    /// Hop index within the turn.
    /// 0 = user message or pre-assistant items
    /// 1+ = which assistant response-continuation cycle this belongs to
    pub hop_index: u32,

    /// Additional metadata (provider-specific, tool names, etc.).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub metadata: BTreeMap<String, Value>,

    /// File references associated with this message.
    /// Populated when a tool reads a large file; propagated through compaction.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_refs: Vec<FileRefId>,
}

impl StoredMessage {
    /// Create a new message to be persisted.
    pub fn new(
        id: MessageId,
        conversation_id: impl Into<String>,
        role: MessageRole,
        content: impl Into<String>,
        token_count: u32,
        timestamp_unix_ms: i64,
        seq: u32,
        hop_index: u32,
    ) -> Self {
        Self {
            id,
            conversation_id: conversation_id.into(),
            role,
            content: content.into(),
            token_count,
            timestamp_unix_ms,
            covered_by: None,
            thinking: None,
            seq,
            hop_index,
            metadata: BTreeMap::new(),
            file_refs: Vec::new(),
        }
    }

    /// Derive a searchable text representation of this message.
    pub fn search_text(&self) -> String {
        format!("{}: {}", self.role, self.content)
    }
}

// ── Summary DAG ─────────────────────────────────────────────────────────────

/// The kind of a summary node.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum SummaryKind {
    /// A leaf summary that directly compresses a span of raw messages.
    Leaf,
    /// A condensed summary that compresses multiple existing summaries
    /// into a higher-order summary.
    Condensed,
}

/// A node in the summary DAG.
///
/// Summary nodes form a directed acyclic graph where:
/// - Leaf nodes point directly to `StoredMessage` IDs
/// - Condensed nodes point to other `SummaryNode` IDs
/// - Every node retains `parent` back-references for upward traversal
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SummaryNode {
    /// Unique identifier for this summary.
    pub id: SummaryId,

    /// The conversation this summary belongs to.
    pub conversation_id: String,

    /// Whether this is a leaf or condensed summary.
    pub kind: SummaryKind,

    /// The summary text — what the LLM sees in the active context.
    pub text: String,

    /// Estimated token count of the summary text.
    pub token_count: u32,

    /// Unix timestamp in milliseconds when this summary was created.
    pub created_at_unix_ms: i64,

    /// The compaction level used to create this summary:
    /// 1 = Normal (preserve details)
    /// 2 = Aggressive (bullet points)
    /// 3 = Truncation (deterministic, no LLM)
    pub compaction_level: u8,

    /// Parent summary nodes (for upward DAG traversal).
    /// A summary can have multiple parents if it's referenced by multiple
    /// higher-order summaries.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub parents: Vec<SummaryId>,

    /// File references associated with messages covered by this summary.
    /// Propagated upward during compaction so the model retains file
    /// awareness even after multiple rounds of summarization.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub file_refs: Vec<FileRefId>,
}

// ── DAG Edges ────────────────────────────────────────────────────────────────

/// A child reference in the DAG — what a summary node covers.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SummaryChild {
    /// The summary directly covers these raw messages.
    Messages { ids: Vec<MessageId> },
    /// The summary condenses these existing summaries.
    Summaries { ids: Vec<SummaryId> },
}

impl SummaryChild {
    /// Returns the number of items covered.
    pub fn len(&self) -> usize {
        match self {
            SummaryChild::Messages { ids } => ids.len(),
            SummaryChild::Summaries { ids } => ids.len(),
        }
    }
}

// ── Conversation Metadata ───────────────────────────────────────────────────

/// Metadata for a conversation stored in the LCM store.
///
/// This replaces the legacy `metadata.json` approach, providing
/// a single source of truth for conversation-level information.
/// Canonical conversation metadata — the single source of truth shared
/// between the LCM (SQLite) store and the legacy JSON-based store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMeta {
    /// Schema version (see `conversation_store::LOG_VERSION`).
    /// Defaults to 0 for LCM-native conversations that predate version tracking.
    #[serde(default)]
    pub version: u32,

    /// Unique conversation identifier.
    pub conversation_id: String,

    /// Human-readable title.
    pub title: String,

    /// Source of the title: "manual", "auto", or "pending".
    pub title_source: String,

    /// When the conversation was created.
    pub created_at_unix_ms: i64,

    /// When the conversation was last updated.
    pub updated_at_unix_ms: i64,

    /// Number of messages in the conversation.
    pub message_count: u32,

    /// Preview text of the last substantive message for sidebar display.
    #[serde(default)]
    pub last_message_preview: String,

    /// Type of conversation ("standard", etc.).
    pub conversation_type: String,

    /// Flexible metadata map for dynamic_tools, mounted_servers, token_usage, etc.
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

impl ConversationMeta {
    /// Create default metadata for a new conversation.
    pub fn new(conversation_id: impl Into<String>, created_at_unix_ms: i64) -> Self {
        Self {
            version: 0,
            conversation_id: conversation_id.into(),
            title: String::new(),
            title_source: "pending".to_string(),
            created_at_unix_ms,
            updated_at_unix_ms: created_at_unix_ms,
            message_count: 0,
            last_message_preview: String::new(),
            conversation_type: "standard".to_string(),
            metadata: BTreeMap::new(),
        }
    }
}

// ── File References ─────────────────────────────────────────────────────────

/// A lightweight reference to a large file encountered during the session.
///
/// Large files (exceeding `large_file_token_threshold`) are never loaded
/// directly into the active context. Instead, they are registered as
/// `FileReference`s with an exploration summary, and the model interacts
/// with them through standard filesystem tools.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileReference {
    /// Unique identifier for this file reference.
    pub id: FileRefId,

    /// The conversation this file belongs to.
    pub conversation_id: String,

    /// Absolute path to the file on disk.
    pub path: String,

    /// MIME type of the file.
    pub mime_type: String,

    /// Estimated token count of the file contents.
    pub token_count: u32,

    /// An exploration summary — a concise description of the file's
    /// structure and contents generated by a type-aware explorer.
    /// This is what the model sees instead of the raw file.
    pub exploration_summary: String,

    /// Unix timestamp in milliseconds when this reference was registered.
    pub registered_at_unix_ms: i64,
}

// ── Active Context ──────────────────────────────────────────────────────────

/// An entry in the active context — the window sent to the LLM.
///
/// The active context is assembled from a mix of recent raw messages
/// and precomputed summary pointers. This enum represents what the
/// model actually sees.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum ContextEntry {
    /// A raw, uncompacted message.
    RawMessage {
        /// The message's unique ID.
        id: MessageId,
        /// The role of the speaker.
        role: MessageRole,
        /// The full message content.
        content: String,
        /// Reasoning / thinking content (chain-of-thought) for this message.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        thinking: Option<String>,
        /// Global sequence number within the conversation (for ordering).
        seq: u32,
        /// Hop index within the turn (0=user, 1+=assistant hops).
        hop_index: u32,
        /// Opaque metadata carried from the StoredMessage (e.g. call_id, tool name).
        /// When present, `context_to_provider_items` uses this to reconstruct
        /// structured `function_call` / `function_call_output` items instead of
        /// emitting a plain `role`-based message.
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        metadata: BTreeMap<String, Value>,
    },
    /// A pointer to a summary node, with the summary text inlined.
    SummaryPointer {
        /// The summary node's ID.
        summary_id: SummaryId,
        /// The summary text (inlined for the model to read).
        text: String,
        /// IDs of the items covered by this summary, enabling the model
        /// to know what it can expand via `lcm_expand`.
        child_ids: Vec<LcmId>,
        /// File references propagated from covered messages.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        file_refs: Vec<FileRefId>,
    },
    /// A pointer to a large file, with its exploration summary inlined.
    FilePointer {
        /// The file reference ID.
        file_id: FileRefId,
        /// The original file path.
        path: String,
        /// The exploration summary for the model to read.
        exploration_summary: String,
    },
}

// ── Search & Retrieval ──────────────────────────────────────────────────────

/// A single search result from `lcm_grep`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GrepResult {
    /// The message that matched the pattern.
    pub message: StoredMessage,
    /// The summary node that currently covers this message, if any.
    pub covered_by_summary: Option<SummaryId>,
    /// The line/position where the match occurred.
    pub match_context: String,
}

/// Paginated grep results to prevent context flooding.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaginatedGrepResults {
    /// The matching results for the current page.
    pub results: Vec<GrepResult>,
    /// Total number of matches found.
    pub total_count: usize,
    /// Whether there are more results beyond this page.
    pub has_more: bool,
    /// Cursor for fetching the next page.
    pub next_cursor: Option<String>,
}

/// Metadata returned by `lcm_describe` for any LCM entity.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "entityType")]
pub enum DescribeResult {
    #[serde(rename = "message")]
    Message {
        id: MessageId,
        role: MessageRole,
        token_count: u32,
        timestamp_unix_ms: i64,
        covered_by: Option<SummaryId>,
    },
    #[serde(rename = "summary")]
    Summary {
        id: SummaryId,
        kind: SummaryKind,
        token_count: u32,
        compaction_level: u8,
        created_at_unix_ms: i64,
        parents: Vec<SummaryId>,
        child_count: usize,
        file_refs: Vec<FileRefId>,
        /// The full summary text (can be large; consider using summary_id
        /// as a pointer in context rather than inlining this).
        text: String,
    },
    #[serde(rename = "file")]
    File {
        id: FileRefId,
        path: String,
        mime_type: String,
        token_count: u32,
        exploration_summary: String,
        registered_at_unix_ms: i64,
    },
}

// ── LCM Configuration ───────────────────────────────────────────────────────

/// Configuration for the LCM engine.
///
/// These thresholds control when and how context compaction occurs.
/// The defaults are chosen based on the paper's recommendations and
/// practical experience with modern LLM context windows.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", default)]
pub struct LcmConfig {
    /// When true, the soft/hard/large-file thresholds are computed dynamically
    /// from the active model's context window. Percentages: soft=50%, hard=85%,
    /// large_file=10% (capped at 100K). When false, the manually-configured
    /// values below are used.
    /// Default: true (dynamic).
    pub dynamic_thresholds: bool,

    /// Soft token threshold: when the active context exceeds this count,
    /// asynchronous compaction is triggered (does not block the user).
    /// Default: 65,536 (64K tokens).
    pub soft_token_threshold: u32,

    /// Hard token threshold: when the active context exceeds this count,
    /// compaction blocks until enough space is freed.
    /// Default: 131,072 (128K tokens).
    pub hard_token_threshold: u32,

    /// Files exceeding this token count are stored as `FileReference`s
    /// with exploration summaries rather than being loaded into context.
    /// Default: 25,600 (25K tokens).
    pub large_file_token_threshold: u32,

    /// Maximum time (in seconds) allowed for an asynchronous compaction
    /// before it is considered failed and retried on the next cycle.
    /// Default: 25 seconds.
    pub compaction_timeout_secs: u32,

    /// Maximum number of messages to compact in a single block.
    /// Default: 20.
    pub max_compact_block_size: usize,

    /// Maximum number of summary levels allowed in the DAG before
    /// forced condensation occurs.
    /// Default: 5.
    pub max_summary_depth: u32,

    /// Maximum token count for Level 3 deterministic truncation.
    /// Default: 128 tokens (~512 characters at 4:1 ratio).
    pub truncation_max_tokens: u32,

    /// Page size for paginated grep results.
    /// Default: 20.
    pub grep_page_size: usize,

    /// Model reference for LLM-powered summarization (Level 1 & 2 compaction).
    /// When empty or "default", uses the app's `utility_small_model`.
    /// Set to a specific model ref like "openai::gpt-4o-mini" to override.
    /// Default: "" (uses utility_small_model).
    #[serde(default)]
    pub summarization_model: String,

    /// Model ID for token counting within the LCM engine.
    /// When set, LCM uses the real HuggingFace tokenizer for accurate token counts.
    /// When `None`, falls back to the 4:1 character heuristic.
    /// Default: None (uses char-based estimation).
    #[serde(default)]
    pub tokenizer_model_id: Option<String>,
}

impl Default for LcmConfig {
    fn default() -> Self {
        Self {
            dynamic_thresholds: true,
            soft_token_threshold: 65536,       // 64K
            hard_token_threshold: 131072,      // 128K
            large_file_token_threshold: 25600, // 25K
            compaction_timeout_secs: 25,
            max_compact_block_size: 20,
            max_summary_depth: 5,
            truncation_max_tokens: 128,
            grep_page_size: 20,
            summarization_model: String::new(),
            tokenizer_model_id: None,
        }
    }
}

impl LcmConfig {
    /// Produce effective thresholds, optionally auto-computed from the model's
    /// context window when `dynamic_thresholds` is enabled.
    ///
    /// Dynamic formulas (when enabled):
    /// - Soft  = 50% of context window
    /// - Hard  = 85% of context window
    /// - Large file = 10% of context window, capped at 100,000 tokens
    ///
    /// When `dynamic_thresholds` is `false`, returns `self` unchanged.
    pub fn with_dynamic_thresholds(self, context_window: usize) -> Self {
        if !self.dynamic_thresholds {
            return self;
        }
        let cw = context_window as u32;
        Self {
            soft_token_threshold: cw / 2,
            hard_token_threshold: (cw as f64 * 0.85) as u32,
            large_file_token_threshold: (cw / 10).min(100_000),
            ..self
        }
    }

    /// Adjust LCM thresholds to reserve space for fixed system prompt overhead.
    ///
    /// System items (prompt composer blocks, temporal context, memory context)
    /// are outside LCM's control but consume the model's context window. This
    /// method subtracts the overhead from the effective window before applying
    /// threshold percentages, ensuring compaction triggers early enough that
    /// the total input (system + LCM context + user) fits the model's limit.
    ///
    /// Example: if the model window is 128K and system overhead is 20K, the
    /// effective window for LCM is 108K, so soft threshold = 108K/2 = 54K
    /// instead of 128K/2 = 64K.
    pub fn with_system_overhead(mut self, system_overhead_tokens: u32) -> Self {
        if !self.dynamic_thresholds {
            return self;
        }
        // Derive the original context window from the existing thresholds.
        let implied_cw = (self.hard_token_threshold as f64 / 0.85) as u32;
        let effective_cw = implied_cw.saturating_sub(system_overhead_tokens);

        // Only clamp if the overhead is meaningful (>1K tokens) and the
        // effective window is still large enough for useful LCM context.
        if system_overhead_tokens > 1024 && effective_cw > 8192 {
            self.soft_token_threshold = effective_cw / 2;
            self.hard_token_threshold = (effective_cw as f64 * 0.85) as u32;
            self.large_file_token_threshold = (effective_cw / 10).min(100_000);
        }
        // For small overheads (<1K), the existing thresholds are fine.
        self
    }
}

// ── Error Type ───────────────────────────────────────────────────────────────

/// Errors that can occur in LCM operations.
#[derive(Debug, thiserror::Error)]
pub enum LcmError {
    #[error("Store error: {0}")]
    Store(String),

    #[error("DAG error: {0}")]
    Dag(String),

    #[error("Entity not found: {0}")]
    NotFound(String),

    #[error("Invalid configuration: {0}")]
    Config(String),

    #[error("Compaction error: {0}")]
    Compaction(String),

    #[error("Concurrency error: {0}")]
    Concurrency(String),

    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),

    #[error("SQL error: {0}")]
    Sql(#[from] rusqlite::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ── Token Estimation ────────────────────────────────────────────────────────

/// Rough token estimation for a string.
///
/// Uses the 4:1 character-to-token heuristic as a fast approximation.
/// For precise counts, the tokenizer module should be used instead.
pub fn estimate_tokens(text: &str) -> u32 {
    // ~4 characters per token for English text
    (text.chars().count() as u32).div_ceil(4)
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lcm_id_creation() {
        let id = LcmId::new();
        assert!(!id.0.is_empty());
    }

    #[test]
    fn test_stored_message_new() {
        let msg = StoredMessage::new(
            LcmId::new(),
            "conv-1",
            MessageRole::User,
            "Hello, world!",
            3,
            1000,
            1,
            0,
        );
        assert_eq!(msg.role, MessageRole::User);
        assert_eq!(msg.content, "Hello, world!");
        assert_eq!(msg.token_count, 3);
        assert!(msg.covered_by.is_none());
    }

    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1); // 4 chars = 1 token
        assert_eq!(estimate_tokens("abcde"), 2); // 5 chars = 2 tokens
    }

    #[test]
    fn test_lcm_config_defaults() {
        let config = LcmConfig::default();
        assert_eq!(config.soft_token_threshold, 65536);
        assert_eq!(config.hard_token_threshold, 131072);
        assert_eq!(config.large_file_token_threshold, 25600);
    }
}
