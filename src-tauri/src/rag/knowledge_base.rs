//! Knowledge Base Manager — global, profile-aware document index.
//!
//! Manages multiple named knowledge bases, each backed by a LanceDB vector
//! store and a SQLite FTS5 keyword store. Knowledge bases are global
//! (stored under `$AGENTJAX_HOME/knowledge_bases/{id}/`) and available to
//! all agent profiles by default. Profiles can individually disable a KB.
//!
//! ## Features
//!
//! - **Hybrid search**: Reciprocal rank fusion of vector (cosine) and
//!   keyword (BM25) results for better retrieval quality.
//! - **Markdown-aware chunking**: Uses `MarkdownChunker` for natural
//!   split points at structural boundaries.
//! - **Content hashing**: Documents are deduplicated by SHA-256 hash.
//! - **Background indexing**: Long-running index operations report progress.

use crate::agentjax_home;
use crate::config::{AppConfig, AgentConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::{self, EmbeddingRequest};
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::RwLock;

use super::chunking::{Chunker, MarkdownChunker};
use super::fts_store::{FtsStore, content_hash};
use super::types::{
    Chunk, Document, HybridSearchResult, IndexingProgress, KnowledgeBaseInfo, SearchConfig,
};
use super::vector_store::VectorStore;

// ── KnowledgeBaseManager ────────────────────────────────────────────────────

/// The global knowledge base manager.
///
/// Holds open connections to all known knowledge bases. Use `open_default()`
/// to initialize from the shared `AppConfig`, then `ensure_kb()` to lazily
/// open specific KBs on first use.
pub struct KnowledgeBaseManager {
    /// Open knowledge bases, keyed by KB ID.
    bases: RwLock<HashMap<String, Arc<KnowledgeBase>>>,
    /// Root directory for KB storage (typically `$AGENTJAX_HOME/knowledge_bases`).
    root_dir: PathBuf,
    /// Embedding provider key.
    provider_key: String,
    /// Embedding model identifier.
    embedding_model: String,
    /// Chunker shared across all KBs.
    #[allow(dead_code)]
    chunker: Chunker,
    /// Default top-K for searches.
    #[allow(dead_code)]
    top_k: usize,
}

impl KnowledgeBaseManager {
    /// Create a new KB manager from app + agent config.
    ///
    /// KBs are stored under `$AGENTJAX_HOME/knowledge_bases/` and use the
    /// agent's embedding provider configuration.
    pub fn from_config(_app_config: &AppConfig, agent_config: &AgentConfig) -> AgentJaxResult<Self> {
        let home = agentjax_home::agentjax_home_dir()?;
        let root_dir = home.join("knowledge_bases");
        let rag = &agent_config.rag;
        let chunker = Chunker::new(
            rag.chunk_size,
            rag.chunk_overlap,
            rag.chunk_window
                .unwrap_or(super::chunking::DEFAULT_WINDOW_CHARS),
        )?;
        let provider_key = rag
            .embedding
            .provider_key
            .clone()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| agent_config.active_provider.clone());
        let embedding_model = rag.embedding.model.clone();

        Ok(Self {
            bases: RwLock::new(HashMap::new()),
            root_dir,
            provider_key,
            embedding_model,
            chunker,
            top_k: rag.top_k,
        })
    }

    // ── KB Management ──────────────────────────────────────────────────

    /// List all available knowledge bases found on disk.
    ///
    /// Scans the KB root directory for subdirectories containing both a
    /// LanceDB database and an FTS database.
    pub async fn list_kbs(&self, agent_config: &AgentConfig) -> AgentJaxResult<Vec<KnowledgeBaseInfo>> {
        let mut infos = Vec::new();

        if !self.root_dir.exists() {
            return Ok(infos);
        }

        let mut entries = tokio::fs::read_dir(&self.root_dir).await.map_err(|e| {
            AgentJaxError::embedding(format!("Failed to read KB directory: {e}"))
        })?;

        while let Some(entry) = entries.next_entry().await.map_err(|e| {
            AgentJaxError::embedding(format!("Failed to read KB entry: {e}"))
        })? {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let kb_id = entry
                .file_name()
                .to_string_lossy()
                .to_string();
            if kb_id.starts_with('.') {
                continue;
            }

            // Check that both stores exist
            let lance_dir = path.join("vectors");
            let fts_path = path.join("fts.db");
            if !lance_dir.exists() || !fts_path.exists() {
                continue;
            }

            // Open the KB to get counts
            match self.open_kb(&kb_id).await {
                Ok(_kb) => {
                    let _doc_ids = _kb.vector_store.list_documents().await.unwrap_or_default();

                    let fts_docs = _kb.fts_store.list_documents().unwrap_or_default();
                    let total_bytes: u64 = fts_docs.iter().map(|d| d.byte_count).sum();

                    // KB name from metadata (stored in a .meta file, fallback to id)
                    let name = kb_id.clone();

                    infos.push(KnowledgeBaseInfo {
                        id: kb_id,
                        name,
                        description: String::new(),
                        document_count: fts_docs.len(),
                        chunk_count: _doc_ids.len(),
                        total_bytes,
                        enabled: agent_config.rag.enabled,
                    });
                }
                Err(e) => {
                    log::warn!("Failed to open KB '{}' for listing: {e}", kb_id);
                }
            }
        }

        Ok(infos)
    }

    /// Open or create a knowledge base by ID.
    ///
    /// Lazily opens the vector and FTS stores. Subsequent calls return the
    /// cached instance.
    pub async fn open_kb(&self, kb_id: &str) -> AgentJaxResult<Arc<KnowledgeBase>> {
        // Fast path: already cached
        {
            let bases = self.bases.read().await;
            if let Some(kb) = bases.get(kb_id) {
                return Ok(kb.clone());
            }
        }

        // Slow path: open on disk
        let kb_dir = self.root_dir.join(kb_id);
        let vector_store = VectorStore::open(kb_dir.join("vectors")).await?;
        let fts_store = FtsStore::open(kb_dir.join("fts.db"))?;

        let kb = Arc::new(KnowledgeBase {
            id: kb_id.to_string(),
            vector_store,
            fts_store,
            chunker: MarkdownChunker::with_defaults(),
        });

        let mut bases = self.bases.write().await;
        bases.insert(kb_id.to_string(), kb.clone());
        Ok(kb)
    }

    // ── Indexing ───────────────────────────────────────────────────────

    /// Index a document into a knowledge base.
    ///
    /// Chunks the document, embeds all chunks via the provider API, and
    /// stores them in both the vector store and FTS store. If the document
    /// already exists (same content hash), it is skipped.
    pub async fn index_document(
        &self,
        kb_id: &str,
        document: Document,
        _app_config: &AppConfig,
    ) -> AgentJaxResult<IndexingProgress> {
        let kb = self.open_kb(kb_id).await?;
        let hash = content_hash(&document.content);
        let total = 1;

        // Check for duplicate content
        {
            let existing = kb.fts_store.list_documents().unwrap_or_default();
            if existing.iter().any(|d| d.content_hash == hash) {
                return Ok(IndexingProgress {
                    kb_id: kb_id.to_string(),
                    total_documents: total,
                    processed: 1,
                    chunks_created: 0,
                    done: true,
                    error: Some("Document with identical content already indexed".to_string()),
                });
            }
        }

        // Chunk
        let chunks = kb.chunker.chunk(&document);
        if chunks.is_empty() {
            return Ok(IndexingProgress {
                kb_id: kb_id.to_string(),
                total_documents: total,
                processed: 1,
                chunks_created: 0,
                done: true,
                error: Some("Document produced no chunks".to_string()),
            });
        }

        let chunk_count = chunks.len();

        // Embed all chunks
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let response = provider_api::embed_text(
            _app_config,
            &self.provider_key,
            &self.embedding_model,
            &EmbeddingRequest::batch(texts),
        )
        .await?;

        if response.embeddings.len() != chunk_count {
            return Err(AgentJaxError::embedding(format!(
                "Expected {} embeddings, got {}",
                chunk_count,
                response.embeddings.len()
            )));
        }

        // Attach embeddings
        let mut embedded_chunks: Vec<Chunk> = chunks;
        for (i, embedding) in response.embeddings.into_iter().enumerate() {
            embedded_chunks[i].embedding = Some(embedding);
        }

        // Store: vector DB
        kb.vector_store.insert_chunks(&embedded_chunks).await?;

        // Store: FTS metadata
        kb.fts_store.upsert_document(&document, &hash)?;

        // Store: FTS chunk text
        kb.fts_store.insert_chunks(&embedded_chunks, &self.embedding_model)?;

        log::info!(
            "Indexed document '{}' into KB '{}': {} chunks",
            document.id,
            kb_id,
            chunk_count
        );

        Ok(IndexingProgress {
            kb_id: kb_id.to_string(),
            total_documents: total,
            processed: 1,
            chunks_created: chunk_count,
            done: true,
            error: None,
        })
    }

    // ── Search ─────────────────────────────────────────────────────────

    /// Hybrid search: combine vector similarity with keyword search.
    ///
    /// Uses reciprocal rank fusion (RRF) to merge results from both stores.
    /// The `top_k` parameter controls the final number of results returned.
    pub async fn search(
        &self,
        kb_id: &str,
        query: &str,
        top_k: usize,
        app_config: &AppConfig,
    ) -> AgentJaxResult<Vec<HybridSearchResult>> {
        let kb = self.open_kb(kb_id).await?;

        // Get the embedding for the query
        let response = provider_api::embed_text(
            app_config,
            &self.provider_key,
            &self.embedding_model,
            &EmbeddingRequest::single(query),
        )
        .await?;

        let query_vector = response.embeddings.into_iter().next().ok_or_else(|| {
            AgentJaxError::embedding("Empty embedding response")
        })?;

        // Run both searches in parallel
        let vec_config = SearchConfig {
            top_k: top_k * 2, // Over-fetch for fusion
            ..Default::default()
        };

        let (vec_results, fts_results) = tokio::join!(
            kb.vector_store.search(&query_vector, &vec_config),
            async { kb.fts_store.search_fts(query, top_k * 2) },
        );

        let vec_results = vec_results.unwrap_or_default();
        let fts_results = fts_results.unwrap_or_default();

        // Reciprocal Rank Fusion
        let k: f32 = 60.0;
        let mut score_map: HashMap<String, (f32, f32, &str, &str, &str, &str, &BTreeMap<String, String>)> =
            HashMap::new();

        // Vector results
        for (rank, r) in vec_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank + 1) as f32);
            score_map.insert(
                r.chunk_id.clone(),
                (rrf, r.score, &r.document_id, &r.content, "", "", &r.metadata),
            );
        }

        // FTS results
        for (rank, r) in fts_results.iter().enumerate() {
            let rrf = 1.0 / (k + (rank + 1) as f32);
            let entry = score_map
                .entry(r.chunk_id.clone())
                .or_insert_with(|| (0.0, 0.0, &r.document_id, &r.content, &r.title, "", &EMPTY_META));
            entry.0 += rrf; // Accumulate RRF score
            entry.1 = entry.1.max(normalize_bm25(r.bm25_score));
            if !r.title.is_empty() {
                entry.4 = &r.title;
            }
        }

        // Sort by fused score
        let mut fused: Vec<(&String, &(f32, f32, &str, &str, &str, &str, &BTreeMap<String, String>))> =
            score_map.iter().collect();
        fused.sort_by(|a, b| b.1 .0.partial_cmp(&a.1 .0).unwrap_or(std::cmp::Ordering::Equal));
        fused.truncate(top_k);

        let results: Vec<HybridSearchResult> = fused
            .into_iter()
            .map(|(chunk_id, (rrf_score, keyword_score, doc_id, content, title, _fts_title, meta))| {
                let vector_score = (rrf_score - keyword_score).max(0.0);
                HybridSearchResult {
                    chunk_id: chunk_id.clone(),
                    document_id: doc_id.to_string(),
                    title: if title.is_empty() { "Untitled".to_string() } else { title.to_string() },
                    content: content.to_string(),
                    score: *rrf_score,
                    vector_score,
                    keyword_score: *keyword_score,
                    metadata: (*meta).clone(),
                }
            })
            .collect();

        Ok(results)
    }

    /// Delete a document from a knowledge base.
    #[allow(dead_code)]
    pub async fn delete_document(&self, kb_id: &str, document_id: &str) -> AgentJaxResult<()> {
        let kb = self.open_kb(kb_id).await?;
        kb.vector_store.delete_document(document_id).await?;
        kb.fts_store.delete_document(document_id)?;
        Ok(())
    }

    /// Get a document's chunks from a knowledge base.
    pub async fn get_document(&self, kb_id: &str, document_id: &str) -> AgentJaxResult<Vec<String>> {
        let kb = self.open_kb(kb_id).await?;
        kb.fts_store.get_document_chunks(document_id)
    }
}

// ── KnowledgeBase ───────────────────────────────────────────────────────────

/// A single knowledge base instance.
///
/// Owns both the vector store (LanceDB) and the FTS store (SQLite).
pub struct KnowledgeBase {
    #[allow(dead_code)]
    pub id: String,
    pub vector_store: VectorStore,
    pub fts_store: FtsStore,
    pub chunker: Chunker,
}

// ── Helpers ─────────────────────────────────────────────────────────────────

/// Normalize BM25 score from FTS5 (raw negative/zero values) to [0, 1).
fn normalize_bm25(raw: f64) -> f32 {
    let abs = raw.abs();
    (abs / (1.0 + abs)) as f32
}

static EMPTY_META: BTreeMap<String, String> = BTreeMap::new();

// ── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_bm25() {
        let score = normalize_bm25(-2.5);
        assert!(score > 0.5);
        assert!(score < 1.0);
    }

    #[test]
    fn test_normalize_bm25_zero() {
        let score = normalize_bm25(0.0);
        assert_eq!(score, 0.0);
    }
}
