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

impl Chunk {
    /// Create a new chunk.
    pub fn new(
        document_id: impl Into<String>,
        chunk_index: usize,
        content: impl Into<String>,
    ) -> Self {
        let content: String = content.into();
        let id = format!("{}_{}", document_id.into(), chunk_index);
        Self {
            id,
            document_id: String::new(),
            content,
            chunk_index,
            embedding: None,
        }
    }
}

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
