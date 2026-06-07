//! Knowledge Base types — data structures specific to the KB application layer.
//!
//! These types are built on top of the generic RAG types (`crate::rag::types`)
//! and add KB-specific metadata like KB IDs, titles, indexing progress, etc.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

// ── Knowledge Base Info ──────────────────────────────────────────────────────

/// Metadata about a registered knowledge base.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KnowledgeBaseInfo {
    /// Unique knowledge base identifier.
    pub id: String,
    /// Human-readable name.
    pub name: String,
    /// Optional description.
    #[serde(default)]
    pub description: String,
    /// Number of documents indexed.
    pub document_count: usize,
    /// Total number of chunks across all documents.
    pub chunk_count: usize,
    /// Total bytes of source content.
    pub total_bytes: u64,
    /// Whether this KB is enabled for the current agent profile.
    pub enabled: bool,
}

// ── Hybrid Search Result ─────────────────────────────────────────────────────

/// Result from a hybrid (keyword + vector) search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HybridSearchResult {
    /// The chunk ID.
    pub chunk_id: String,
    /// The parent document ID.
    pub document_id: String,
    /// Document title (extracted from heading or metadata).
    #[serde(default)]
    pub title: String,
    /// The chunk text content.
    pub content: String,
    /// Combined hybrid score (0.0 - 1.0, higher = better).
    pub score: f32,
    /// Vector similarity component of the score.
    #[serde(default)]
    pub vector_score: f32,
    /// Keyword (FTS BM25) component of the score.
    #[serde(default)]
    pub keyword_score: f32,
    /// Arbitrary metadata from the parent document.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

// ── Indexing Progress ────────────────────────────────────────────────────────

/// Knowledge base indexing progress for long-running operations.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IndexingProgress {
    /// The knowledge base being indexed.
    pub kb_id: String,
    /// Total documents to process.
    pub total_documents: usize,
    /// Documents processed so far.
    pub processed: usize,
    /// Total chunks created so far.
    pub chunks_created: usize,
    /// Whether the indexing operation is complete.
    pub done: bool,
    /// Error message if indexing failed.
    #[serde(default)]
    pub error: Option<String>,
}
