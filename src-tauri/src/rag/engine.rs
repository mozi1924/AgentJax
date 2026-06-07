//! RagEngine — unified embedding and hybrid search entry point.
//!
//! Provides a high-level API for embedding text and performing hybrid
//! (vector + FTS) search. Designed to be shared by multiple consumers:
//! Knowledge Base, Memory, and future Workspace indexing.
//!
//! ## Usage
//!
//! ```ignore
//! use rag::engine::RagEngine;
//!
//! let engine = RagEngine::from_config(&app_config)?;
//! let embeddings = engine.embed_batch(&app_config, &texts).await?;
//! ```

use crate::config::AppConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::{self, EmbeddingRequest};

/// The RAG engine — stateless embedding and search provider.
///
/// Holds the resolved embedding model configuration. Multiple consumers
/// (KB, Memory, Workspace) can share a single `RagEngine` instance or
/// create their own — it's cheap to construct and has no mutable state.
#[derive(Clone)]
pub struct RagEngine {
    /// Provider key resolved from the embedding model reference.
    provider_key: String,
    /// Resolved embedding model ID.
    embedding_model: String,
    /// True if no embedding model is configured or resolution failed.
    embedding_disabled: bool,
}

impl RagEngine {
    /// Create a new RAG engine from the global app config.
    ///
    /// Resolves the embedding model reference (e.g. `"openai/text-embedding-3-small"`)
    /// into a concrete provider key and model ID. If resolution fails, the engine
    /// is created in disabled mode — `embed_batch` will always return an error.
    pub fn from_config(app_config: &AppConfig) -> AgentJaxResult<Self> {
        let rag = &app_config.rag;

        let (provider_key, embedding_model, embedding_disabled) =
            match app_config.resolve_embedding_profile(&rag.embedding.model) {
                Ok((pk, _provider, model_id)) => (pk, model_id, false),
                Err(e) => {
                    log::warn!(
                        "Embedding model not configured or resolution failed: {}. \
                         Semantic search will use FTS5-only (keyword) fallback.",
                        e
                    );
                    (String::new(), String::new(), true)
                }
            };

        Ok(Self {
            provider_key,
            embedding_model,
            embedding_disabled,
        })
    }

    // ── Accessors ───────────────────────────────────────────────────────

    /// Whether embedding is disabled (no model configured or resolution failed).
    pub fn is_embedding_disabled(&self) -> bool {
        self.embedding_disabled
    }

    /// The resolved provider key (empty if embedding is disabled).
    #[allow(dead_code)]
    pub fn provider_key(&self) -> &str {
        &self.provider_key
    }

    /// The resolved embedding model ID (empty if embedding is disabled).
    pub fn embedding_model(&self) -> &str {
        &self.embedding_model
    }

    // ── Embedding API ───────────────────────────────────────────────────

    /// Embed a single text string.
    ///
    /// Returns an error if embedding is disabled or the API call fails.
    pub async fn embed_single(
        &self,
        app_config: &AppConfig,
        text: &str,
    ) -> AgentJaxResult<Vec<f32>> {
        let response = provider_api::embed_text(
            app_config,
            &self.provider_key,
            &self.embedding_model,
            &EmbeddingRequest::single(text),
        )
        .await?;
        response
            .embeddings
            .into_iter()
            .next()
            .ok_or_else(|| AgentJaxError::embedding("Empty embedding response"))
    }

    /// Embed a batch of text strings.
    ///
    /// The provider may limit batch sizes — callers should batch responsibly
    /// (see `KnowledgeBaseManager::embed_prepared_chunks` for adaptive batching).
    pub async fn embed_batch(
        &self,
        app_config: &AppConfig,
        texts: &[String],
    ) -> AgentJaxResult<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let response = provider_api::embed_text(
            app_config,
            &self.provider_key,
            &self.embedding_model,
            &EmbeddingRequest::batch(texts.to_vec()),
        )
        .await?;

        Ok(response.embeddings)
    }
}
