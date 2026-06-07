//! Core data types for the RAG system.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// A document to be indexed into the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Document {
    /// Unique identifier for this document.
    pub id: String,
    /// The full text content of the document.
    pub content: String,
    /// Arbitrary key-value metadata attached to the document.
    #[serde(default)]
    pub metadata: BTreeMap<String, String>,
}

/// A single chunk of a document after text splitting.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Chunk {
    /// Unique identifier for this chunk (e.g., `{doc_id}_{index}`).
    pub id: String,
    /// The parent document ID.
    pub document_id: String,
    /// The text content of this chunk.
    pub content: String,
    /// Zero-based index within the parent document.
    pub chunk_index: usize,
    /// Optional precomputed embedding vector.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub embedding: Option<Vec<f32>>,
}

impl Chunk {}

/// A search result returned by the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchResult {
    /// The chunk ID that was matched.
    pub chunk_id: String,
    /// The parent document ID.
    pub document_id: String,
    /// The text content of the matched chunk.
    pub content: String,
    /// The similarity score (higher = more similar).
    pub score: f32,
    /// Arbitrary metadata from the parent document.
    pub metadata: BTreeMap<String, String>,
}

/// Configuration for vector similarity search.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SearchConfig {
    /// Number of results to return.
    pub top_k: usize,
    /// Minimum similarity score threshold (0.0 to 1.0).
    pub min_score: f32,
    /// Optional filter on metadata keys/values.
    /// Format: `"field = 'value'"` or `"field IN ('v1', 'v2')"`
    pub filter: Option<String>,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            top_k: 5,
            min_score: 0.0,
            filter: None,
        }
    }
}

// ── Knowledge Base Types ────────────────────────────────────────────────────

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
