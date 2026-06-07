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
use crate::config::{AppConfig, KnowledgeBaseEntry};
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
    /// Whether embedding is available (model reference could not be resolved).
    embedding_disabled: bool,
    /// Chunker shared across all KBs.
    #[allow(dead_code)]
    chunker: Chunker,
    /// Default top-K for searches.
    #[allow(dead_code)]
    top_k: usize,
    /// Configured knowledge base entries from the global config.
    /// Used for per-agent disabled_agents gating at runtime.
    knowledge_base_entries: BTreeMap<String, KnowledgeBaseEntry>,
}

impl KnowledgeBaseManager {
    /// Return a reference to the root directory for KB storage.
    pub fn root_dir(&self) -> &std::path::Path {
        &self.root_dir
    }

    /// Whether embedding is disabled (no model configured or resolution failed).
    pub fn is_embedding_disabled(&self) -> bool {
        self.embedding_disabled
    }

    /// Check how many unembedded chunks exist for a KB.
    ///
    /// Returns 0 if the KB doesn't exist or has no unembedded chunks.
    /// Used by the refresh command to decide whether to resume embedding
    /// after an incremental prepare that added no new chunks.
    pub async fn unembedded_chunk_count(&self, kb_id: &str) -> AgentJaxResult<usize> {
        match self.open_kb(kb_id).await {
            Ok(kb) => kb.fts_store.unembedded_chunk_count(),
            Err(_) => Ok(0),
        }
    }

    /// Create a new KB manager from the global app config.
    ///
    /// KBs are stored under `$AGENTJAX_HOME/knowledge_bases/` and use the
    /// global embedding provider configuration from `AppConfig`.
    pub fn from_config(app_config: &AppConfig) -> AgentJaxResult<Self> {
        let home = agentjax_home::agentjax_home_dir()?;
        let root_dir = home.join("knowledge_bases");
        let rag = &app_config.rag;
        let chunker = Chunker::new(
            rag.chunk_size,
            rag.chunk_overlap,
            rag.chunk_window
                .unwrap_or(super::chunking::DEFAULT_WINDOW_CHARS),
        )?;

        // Resolve the embedding model reference. If no embedding model is
        // configured or resolution fails, we disable embedding and fall back
        // to FTS5-only search.
        let (provider_key, embedding_model, embedding_disabled) =
            match app_config.resolve_embedding_profile(&rag.embedding.model) {
                Ok((pk, _provider, model_id)) => (pk, model_id, false),
                Err(e) => {
                    log::warn!(
                        "Embedding model not configured or resolution failed: {}. \
                         KB search will use FTS5-only (keyword) fallback.",
                        e
                    );
                    (String::new(), String::new(), true)
                }
            };

        Ok(Self {
            bases: RwLock::new(HashMap::new()),
            root_dir,
            provider_key,
            embedding_model,
            embedding_disabled,
            chunker,
            top_k: rag.top_k,
            knowledge_base_entries: rag.knowledge_bases.clone(),
        })
    }

    // ── KB Management ──────────────────────────────────────────────────

    /// List all available knowledge bases found on disk.
    ///
    /// Scans the KB root directory for subdirectories containing both a
    /// LanceDB database and an FTS database.
    pub async fn list_kbs(&self, app_config: &AppConfig) -> AgentJaxResult<Vec<KnowledgeBaseInfo>> {
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
                        enabled: app_config.rag.enabled,
                    });
                }
                Err(e) => {
                    log::warn!("Failed to open KB '{}' for listing: {e}", kb_id);
                }
            }
        }

        Ok(infos)
    }

    /// List knowledge bases, filtering out disabled_agents.
    pub async fn list_kbs_filtered(
        &self,
        app_config: &AppConfig,
        agent_id: &str,
    ) -> AgentJaxResult<Vec<KnowledgeBaseInfo>> {
        let kbs = self.list_kbs(app_config).await?;
        Ok(kbs
            .into_iter()
            .filter(|kb| self.is_kb_accessible(&kb.id, agent_id))
            .collect())
    }

    /// Check whether an agent is allowed to access a KB by disabled_agents config.
    ///
    /// Returns `false` for unknown KB IDs — if the KB is not in the config
    /// it does not exist and should not be accessible.
    pub fn is_kb_accessible(&self, kb_id: &str, agent_id: &str) -> bool {
        match self.knowledge_base_entries.get(kb_id) {
            Some(entry) => !entry.disabled_agents.iter().any(|a| a == agent_id),
            None => false,
        }
    }

    /// Check whether a KB ID is known (exists in the config).
    pub fn kb_exists(&self, kb_id: &str) -> bool {
        self.knowledge_base_entries.contains_key(kb_id)
    }

    /// Open or create a knowledge base by ID.
    ///
    /// Lazily opens the vector and FTS stores. Subsequent calls return the
    /// cached instance.
    ///
    /// Returns an error if `kb_id` is not a configured knowledge base —
    /// this prevents accidentally creating empty KB directories on disk
    /// when searching/getting documents from a non-existent KB.
    pub async fn open_kb(&self, kb_id: &str) -> AgentJaxResult<Arc<KnowledgeBase>> {
        // Guard: reject unknown KB IDs before touching the filesystem.
        if !self.kb_exists(kb_id) {
            return Err(AgentJaxError::not_found(format!(
                "Knowledge base '{}' is not configured. Add it in Settings → Knowledge Bases first.",
                kb_id
            )));
        }
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
        kb.fts_store.upsert_document(&document, &hash, 0)?;

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

    /// Re-index a document: delete old chunks then re-index.
    ///
    /// Used by the file watcher for incremental updates when a .md file
    /// changes on disk. First deletes the existing document (if any),
    /// then indexes the new content.
    pub async fn reindex_document(
        &self,
        kb_id: &str,
        document: Document,
        app_config: &AppConfig,
    ) -> AgentJaxResult<IndexingProgress> {
        // Delete old chunks if the document exists.
        if let Ok(kb) = self.open_kb(kb_id).await {
            let _ = kb.vector_store.delete_document(&document.id).await;
            let _ = kb.fts_store.delete_document(&document.id);
        }
        // Index the new content.
        self.index_document(kb_id, document, app_config).await
    }

    // ── Phased Indexing ────────────────────────────────────────────────

    /// Phase 1+2: Prepare — chunk all documents and store in FTS.
    ///
    /// This is a fast, local-only operation. Documents are chunked and
    /// their text is stored in the SQLite FTS store with an empty
    /// `embeddings_model` flag. The FTS5 index is rebuilt afterwards.
    ///
    /// ## Incremental logic
    ///
    /// Each entry is `(doc_id, content, modified_at)`. The method:
    ///
    /// 1. Compares `(content_hash, modified_at)` with stored values.
    ///    If both match → skip (unchanged). If either differs → delete
    ///    old chunks and re-chunk.
    /// 2. After processing, deletes any documents from the store whose
    ///    IDs are no longer in the incoming list (file was deleted).
    ///
    /// The `on_progress` callback receives `(processed, total, doc_id)`
    /// after each document is handled (including skipped ones).
    ///
    /// Returns the total number of chunks prepared.
    pub async fn prepare_kb<F>(
        &self,
        kb_id: &str,
        documents: &[(String, String, i64)], // (doc_id, content, modified_at)
        mut on_progress: F,
    ) -> AgentJaxResult<usize>
    where
        F: FnMut(usize, usize, &str),
    {
        use std::collections::HashSet;

        let kb = self.open_kb(kb_id).await?;
        let mut total_chunks = 0usize;

        // Build lookup of existing documents: id → (content_hash, modified_at)
        let existing: Vec<(String, String, i64)> =
            kb.fts_store.list_document_ids_with_hashes()?;
        let existing_map: std::collections::HashMap<&str, (&str, i64)> = existing
            .iter()
            .map(|(id, hash, mtime)| (id.as_str(), (hash.as_str(), *mtime)))
            .collect();

        // Track which incoming document IDs are actually processed
        let incoming_ids: HashSet<&str> = documents.iter().map(|(id, _, _)| id.as_str()).collect();

        let total_docs = documents.len();
        for (idx, (doc_id, content, modified_at)) in documents.iter().enumerate() {
            let hash = content_hash(content);

            // Report progress for every document (including skipped ones).
            on_progress(idx, total_docs, doc_id);

            // Check if this document has already been indexed with the same
            // content AND the same modification time — skip if unchanged.
            if let Some((existing_hash, existing_mtime)) = existing_map.get(doc_id.as_str()) {
                if *existing_hash == hash && *existing_mtime == *modified_at {
                    continue;
                }
                // Content or mtime changed — delete old chunks before re-indexing.
                let _ = kb.fts_store.delete_chunks_for_document(doc_id);
            }

            let doc = Document {
                id: doc_id.clone(),
                content: content.clone(),
                metadata: std::collections::BTreeMap::new(),
            };

            // Chunk
            let chunks = kb.chunker.chunk(&doc);
            if chunks.is_empty() {
                continue;
            }

            total_chunks += chunks.len();

            // Store document metadata (with modification timestamp)
            kb.fts_store.upsert_document(&doc, &hash, *modified_at)?;

            // Store chunk text (without embeddings yet — embeddings_model='')
            kb.fts_store.prepare_chunks(&chunks)?;
        }

        // Remove documents that are no longer present on disk.
        for (existing_id, _, _) in &existing {
            if !incoming_ids.contains(existing_id.as_str()) {
                log::info!(
                    "KB '{}': removing deleted document '{}'",
                    kb_id,
                    existing_id
                );
                let _ = kb.fts_store.delete_document(existing_id);
                let _ = kb.vector_store.delete_document(existing_id).await;
            }
        }

        // Rebuild FTS index to include all prepared chunks
        kb.fts_store.rebuild_fts()?;

        log::info!(
            "Prepared KB '{}': {} chunks ready for embedding",
            kb_id,
            total_chunks
        );

        Ok(total_chunks)
    }

    /// Phase 3: Embed — pipeline concurrent embedding calls.
    ///
    /// Pre-reads all unembedded chunks, splits them into batches, then fires
    /// off up to `rag.embedding_concurrency` embedding API calls at once.
    /// Each task independently embeds its batch, writes vectors to LanceDB,
    /// and marks chunks as done in FTS.  While one batch is waiting on the
    /// network, another is already in flight — total wall-clock time drops
    /// significantly.
    ///
    /// ## Anti-overload behaviour (per-task)
    ///
    /// - Adaptive shrinking on failure (split batch in half, min floor 5).
    /// - Progressive backoff at min size: 5s → 10s → 20s.
    /// - Non-retryable errors (auth, config) abort all in-flight work.
    /// - `rag.embedding_batch_throttle_ms` still applies between firing
    ///   new tasks to avoid flooding the server.
    ///
    /// The `on_progress` callback receives `(processed, total)` whenever
    /// any task completes its batch.
    pub async fn embed_prepared_chunks<F>(
        &self,
        kb_id: &str,
        app_config: &AppConfig,
        on_progress: F,
    ) -> AgentJaxResult<usize>
    where
        F: FnMut(usize, usize) + Send + 'static,
    {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::{Arc, Mutex};
        use tokio::sync::Semaphore;

        let kb = self.open_kb(kb_id).await?;
        let total = kb.fts_store.unembedded_chunk_count()?;

        if total == 0 {
            return Ok(0);
        }

        let max_batch = app_config.rag.embedding_batch_size.max(5);
        let concurrency = app_config.rag.embedding_concurrency.max(1);
        let throttle = std::time::Duration::from_millis(
            app_config.rag.embedding_batch_throttle_ms,
        );

        // ── Pre-read all batches ──────────────────────────────────────
        let mut batches: Vec<Vec<(String, String, usize, String)>> = Vec::new();
        let mut offset = 0usize;
        while offset < total {
            let batch = kb.fts_store.get_unembedded_chunks(max_batch, offset)?;
            if batch.is_empty() {
                break;
            }
            offset += batch.len();
            batches.push(batch);
        }

        let batch_count = batches.len();
        log::info!(
            "KB '{}': embedding {} chunks in {} batches (concurrency={})",
            kb_id,
            total,
            batch_count,
            concurrency,
        );

        // ── Shared state (Arc for cross-task ownership) ──────────────
        let kb = Arc::new(kb);
        // Clone the KnowledgeBase Arc — the inner stores are already behind
        // an Arc, so this is just a ref-count bump.
        let kb_for_tasks = kb.as_ref().clone(); // Arc<KnowledgeBase>
        let app_config = Arc::new(app_config.clone());
        let provider_key = Arc::new(self.provider_key.clone());
        let embedding_model = Arc::new(self.embedding_model.clone());
        let embedded = Arc::new(AtomicUsize::new(0));
        let first_error: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        let on_progress = Arc::new(Mutex::new(on_progress));
        let sem = Arc::new(Semaphore::new(concurrency));
        let mut join_set = tokio::task::JoinSet::new();

        for (batch_idx, batch) in batches.into_iter().enumerate() {
            // Check if any previous task failed fatally.
            {
                let err_guard = first_error.lock().unwrap();
                if let Some(ref msg) = *err_guard {
                    return Err(AgentJaxError::embedding(msg.clone()));
                }
            }

            // Acquire concurrency slot — blocks if we're at the limit.
            let permit = sem.clone().acquire_owned().await.map_err(|e| {
                AgentJaxError::embedding(format!("Semaphore closed: {e}"))
            })?;

            let kb = kb_for_tasks.clone();
            let app_config = app_config.clone();
            let provider_key = provider_key.clone();
            let embedding_model = embedding_model.clone();
            let embedded = embedded.clone();
            let first_error = first_error.clone();
            let on_progress = on_progress.clone();
            let total_u = total;

            join_set.spawn(async move {
                let _permit = permit; // hold until task completes

                // Embed this batch with adaptive retry/shrink.
                let result = Self::embed_batch_with_retry(
                    &kb,
                    &app_config,
                    &provider_key,
                    &embedding_model,
                    &batch,
                )
                .await;

                match result {
                    Ok(()) => {
                        let done = embedded.fetch_add(batch.len(), Ordering::Relaxed)
                            + batch.len();
                        if let Ok(mut cb) = on_progress.lock() {
                            cb(done, total_u);
                        }
                    }
                    Err(e) => {
                        // Record first error to abort other tasks.
                        let mut err_guard = first_error.lock().unwrap();
                        if err_guard.is_none() {
                            *err_guard = Some(format!(
                                "Batch {} failed: {}",
                                batch_idx,
                                e
                            ));
                        }
                    }
                }
            });

            // Throttle between firing new tasks (NOT between completions).
            if batch_idx + 1 < batch_count {
                tokio::time::sleep(throttle).await;
            }
        }

        // ── Wait for all tasks ────────────────────────────────────────
        while let Some(result) = join_set.join_next().await {
            if let Err(join_err) = result {
                log::warn!("Embedding task panicked: {join_err}");
            }
        }

        let final_embedded = embedded.load(Ordering::Relaxed);

        // Check if any task reported a fatal error.
        let err_guard = first_error.lock().unwrap();
        if let Some(ref msg) = *err_guard {
            return Err(AgentJaxError::embedding(format!(
                "Embedding pipeline failed after {}/{} chunks: {msg}",
                final_embedded, total,
            )));
        }

        log::info!(
            "Embedded {} chunks in KB '{}'",
            final_embedded,
            kb_id,
        );

        Ok(final_embedded)
    }

    /// Embed a single batch with adaptive shrinking and retry.
    ///
    /// Called from within a tokio task — each task owns its batch and
    /// handles its own retries independently.
    async fn embed_batch_with_retry(
        kb: &KnowledgeBase,
        app_config: &AppConfig,
        provider_key: &str,
        embedding_model: &str,
        batch: &[(String, String, usize, String)],
    ) -> AgentJaxResult<()> {
        const MIN_BATCH: usize = 5;
        const MAX_RETRIES: u32 = 3;

        let texts: Vec<String> = batch.iter().map(|(_, _, _, c)| c.clone()).collect();
        let chunk_ids: Vec<String> = batch.iter().map(|(id, _, _, _)| id.clone()).collect();

        // ── Attempt 1: full batch ──
        match provider_api::embed_text(
            app_config,
            provider_key,
            embedding_model,
            &EmbeddingRequest::batch(texts.clone()),
        )
        .await
        {
            Ok(response) => {
                Self::commit_batch(kb, batch, &chunk_ids, &response.embeddings).await?;
                return Ok(());
            }
            Err(e) if matches!(e.kind, crate::error::ErrorKind::ProviderAuth | crate::error::ErrorKind::Config) => {
                return Err(e);
            }
            Err(_) => { /* fall through to shrink/retry */ }
        }

        // ── Shrink ──
        if batch.len() > MIN_BATCH {
            let mid = batch.len() / 2;
            log::warn!(
                "Embedding batch of {} failed — shrinking to {} + {}",
                batch.len(), mid, batch.len() - mid,
            );
            tokio::time::sleep(std::time::Duration::from_secs(3)).await;
            Box::pin(Self::embed_batch_with_retry(
                kb, app_config, provider_key, embedding_model, &batch[..mid],
            ))
            .await?;
            Box::pin(Self::embed_batch_with_retry(
                kb, app_config, provider_key, embedding_model, &batch[mid..],
            ))
            .await?;
            return Ok(());
        }

        // ── Min-size: retry with backoff ──
        for attempt in 0..MAX_RETRIES {
            let delay = 5u64 * 2u64.saturating_pow(attempt);
            log::warn!(
                "Embedding min-batch ({} chunks) retry {}/{} after {}s...",
                batch.len(), attempt + 1, MAX_RETRIES, delay,
            );
            tokio::time::sleep(std::time::Duration::from_secs(delay)).await;

            match provider_api::embed_text(
                app_config,
                provider_key,
                embedding_model,
                &EmbeddingRequest::batch(texts.clone()),
            )
            .await
            {
                Ok(response) => {
                    Self::commit_batch(kb, batch, &chunk_ids, &response.embeddings).await?;
                    return Ok(());
                }
                Err(e) if matches!(e.kind, crate::error::ErrorKind::ProviderAuth | crate::error::ErrorKind::Config) => {
                    return Err(e);
                }
                Err(_) => {}
            }
        }

        Err(AgentJaxError::embedding(format!(
            "Failed after {} retries at min batch size ({})",
            MAX_RETRIES, batch.len(),
        )))
    }

    /// Write embedded chunks to LanceDB and mark them in FTS.
    async fn commit_batch(
        kb: &KnowledgeBase,
        batch: &[(String, String, usize, String)],
        chunk_ids: &[String],
        embeddings: &[Vec<f32>],
    ) -> AgentJaxResult<()> {
        if embeddings.len() != batch.len() {
            return Err(AgentJaxError::embedding(format!(
                "Expected {} embeddings, got {}",
                batch.len(),
                embeddings.len()
            )));
        }

        let embedded_chunks: Vec<Chunk> = batch
            .iter()
            .zip(embeddings.iter())
            .map(|((id, doc_id, idx, content), emb)| Chunk {
                id: id.clone(),
                document_id: doc_id.clone(),
                chunk_index: *idx,
                content: content.clone(),
                embedding: Some(emb.clone()),
            })
            .collect();

        kb.vector_store.insert_chunks(&embedded_chunks).await?;
        // FTS mark — use empty string for model since we're inside a static method.
        // The model name is only used as a tag; pass a placeholder or the actual model.
        kb.fts_store.mark_chunks_embedded(chunk_ids, "pipeline")?;

        Ok(())
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

        // Try to get a query embedding. If embedding is disabled or fails,
        // fall back to FTS5-only search.
        let maybe_query_vector = if self.embedding_disabled {
            log::debug!("Embedding disabled — using FTS5-only search");
            None
        } else {
            match provider_api::embed_text(
                app_config,
                &self.provider_key,
                &self.embedding_model,
                &EmbeddingRequest::single(query),
            )
            .await
            {
                Ok(response) => Some(response.embeddings.into_iter().next().ok_or_else(
                    || AgentJaxError::embedding("Empty embedding response"),
                )?),
                Err(e) => {
                    log::warn!(
                        "Embedding failed for KB search, falling back to FTS5-only: {}",
                        e
                    );
                    None
                }
            }
        };

        match maybe_query_vector {
            Some(query_vector) => {
                // ── Hybrid: vector + FTS5 with RRF fusion ──
                let vec_config = SearchConfig {
                    top_k: top_k * 2,
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
                let mut score_map: HashMap<
                    String,
                    (f32, f32, &str, &str, &str, &str, &BTreeMap<String, String>),
                > = HashMap::new();

                for (rank, r) in vec_results.iter().enumerate() {
                    let rrf = 1.0 / (k + (rank + 1) as f32);
                    score_map.insert(
                        r.chunk_id.clone(),
                        (rrf, r.score, &r.document_id, &r.content, "", "", &r.metadata),
                    );
                }

                for (rank, r) in fts_results.iter().enumerate() {
                    let rrf = 1.0 / (k + (rank + 1) as f32);
                    let entry = score_map.entry(r.chunk_id.clone()).or_insert_with(|| {
                        (0.0, 0.0, &r.document_id, &r.content, &r.title, "", &EMPTY_META)
                    });
                    entry.0 += rrf;
                    entry.1 = entry.1.max(normalize_bm25(r.bm25_score));
                    if !r.title.is_empty() {
                        entry.4 = &r.title;
                    }
                }

                let mut fused: Vec<(
                    &String,
                    &(f32, f32, &str, &str, &str, &str, &BTreeMap<String, String>),
                )> = score_map.iter().collect();
                fused.sort_by(|a, b| {
                    b.1 .0
                        .partial_cmp(&a.1 .0)
                        .unwrap_or(std::cmp::Ordering::Equal)
                });
                fused.truncate(top_k);

                let results: Vec<HybridSearchResult> = fused
                    .into_iter()
                    .map(
                        |(
                            chunk_id,
                            (rrf_score, keyword_score, doc_id, content, title, _fts_title, meta),
                        )| {
                            let vector_score = (rrf_score - keyword_score).max(0.0);
                            HybridSearchResult {
                                chunk_id: chunk_id.clone(),
                                document_id: doc_id.to_string(),
                                title: if title.is_empty() {
                                    "Untitled".to_string()
                                } else {
                                    title.to_string()
                                },
                                content: content.to_string(),
                                score: *rrf_score,
                                vector_score,
                                keyword_score: *keyword_score,
                                metadata: (*meta).clone(),
                            }
                        },
                    )
                    .collect();

                Ok(results)
            }
            None => {
                // ── FTS5-only fallback ──
                let fts_results = kb.fts_store.search_fts(query, top_k)?;
                let results: Vec<HybridSearchResult> = fts_results
                    .into_iter()
                    .map(|r| {
                        let kw = normalize_bm25(r.bm25_score);
                        HybridSearchResult {
                            chunk_id: r.chunk_id,
                            document_id: r.document_id,
                            title: if r.title.is_empty() {
                                "Untitled".to_string()
                            } else {
                                r.title
                            },
                            content: r.content,
                            score: kw,
                            vector_score: 0.0,
                            keyword_score: kw,
                            metadata: BTreeMap::new(),
                        }
                    })
                    .collect();

                Ok(results)
            }
        }
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
