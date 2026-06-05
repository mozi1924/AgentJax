//! High-level RAG indexing and search operations.
//!
//! Coordinates chunking, embedding, and vector store operations into
//! a single `RagIndex` interface.

use crate::config::{AppConfig, RagConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::agentjax_home;
use crate::provider_api;
use crate::provider_api::types::EmbeddingRequest;

use super::chunking::Chunker;
use super::types::{Document, SearchConfig, SearchResult};
use super::vector_store::VectorStore;

/// The main RAG index interface.
///
/// Coordinates chunking, embedding, and vector store operations.
pub struct RagIndex {
    /// The vector store for chunk persistence.
    store: VectorStore,
    /// Text chunker for splitting documents.
    chunker: Chunker,
    /// Search configuration defaults.
    top_k: usize,
    /// Provider key for embedding resolution.
    provider_key: String,
    /// Model ID for embedding.
    embedding_model: String,
}

impl RagIndex {
    /// Create a new `RagIndex` from the app configuration.
    ///
    /// Opens the vector store and initializes the chunker from the
    /// configured RAG settings. Embedding is performed via the
    /// unified `provider_api` protocol layer.
    pub async fn from_config(config: &RagConfig, default_provider_key: &str) -> AgentJaxResult<Self> {
        let home = agentjax_home::agentjax_home_dir()?;
        let store_path = home.join(&config.storage_path);
        let store = VectorStore::open(&store_path).await?;
        let chunker = Chunker::new(config.chunk_size, config.chunk_overlap)?;

        // Resolve the provider key and model for embedding from the config.
        // If `embedding.provider_key` is set, use it; otherwise default to
        // the provided fallback (typically the agent's active_provider).
        let provider_key = config.embedding.provider_key.clone()
            .filter(|k| !k.is_empty())
            .unwrap_or_else(|| default_provider_key.to_string());
        let embedding_model = config.embedding.model.clone();

        Ok(Self {
            store,
            chunker,
            top_k: config.top_k,
            provider_key,
            embedding_model,
        })
    }

    /// Create a `RagIndex` with explicit dependencies (useful for testing).
    pub async fn new(
        store: VectorStore,
        chunker: Chunker,
        top_k: usize,
        provider_key: String,
        embedding_model: String,
    ) -> Self {
        Self {
            store,
            chunker,
            top_k,
            provider_key,
            embedding_model,
        }
    }

    /// Index a document: chunk, embed all chunks, store in vector store.
    pub async fn index_document(
        &self,
        document: Document,
        app_config: &AppConfig,
    ) -> AgentJaxResult<()> {
        // 1. Chunk the document
        let chunks = self.chunker.chunk(&document);
        if chunks.is_empty() {
            return Ok(());
        }

        log::info!(
            "Indexing document '{}' ({} chars, {} chunks)",
            document.id,
            document.content.len(),
            chunks.len()
        );

        // 2. Embed all chunks via provider_api
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();
        let response = provider_api::embed_text(
            app_config,
            &self.provider_key,
            &self.embedding_model,
            &EmbeddingRequest::batch(texts),
        )
        .await?;

        if response.embeddings.len() != chunks.len() {
            return Err(AgentJaxError::embedding(format!(
                "Embedding provider returned {} embeddings for {} chunks",
                response.embeddings.len(),
                chunks.len()
            )));
        }

        // 3. Attach embeddings to chunks
        let mut embedded_chunks: Vec<super::types::Chunk> = chunks;
        for (i, embedding) in response.embeddings.into_iter().enumerate() {
            embedded_chunks[i].embedding = Some(embedding);
        }

        // 4. Store in vector store
        self.store.insert_chunks(&embedded_chunks).await?;

        log::info!("Document '{}' indexed successfully", document.id);
        Ok(())
    }

    /// Search the index with a text query.
    ///
    /// Embeds the query text, then performs ANN search in the vector store.
    pub async fn search(
        &self,
        query_text: &str,
        config: Option<SearchConfig>,
        app_config: &AppConfig,
    ) -> AgentJaxResult<Vec<SearchResult>> {
        let config = config.unwrap_or_else(|| SearchConfig {
            top_k: self.top_k,
            ..Default::default()
        });

        // Embed the query via provider_api
        let response = provider_api::embed_text(
            app_config,
            &self.provider_key,
            &self.embedding_model,
            &EmbeddingRequest::single(query_text),
        )
        .await?;

        let query_vector = response.embeddings.first()
            .ok_or_else(|| AgentJaxError::embedding("Empty embedding response"))?
            .clone();

        // Search the vector store
        let results = self.store.search(&query_vector, &config).await?;
        Ok(results)
    }

    /// Delete all chunks belonging to a document.
    pub async fn delete_document(&self, document_id: &str) -> AgentJaxResult<()> {
        log::info!("Deleting document '{}' from RAG index", document_id);
        self.store.delete_document(document_id).await
    }

    /// List all document IDs in the index.
    pub async fn list_documents(&self) -> AgentJaxResult<Vec<String>> {
        self.store.list_documents().await
    }

    /// Whether the RAG index is empty.
    pub async fn is_empty(&self) -> AgentJaxResult<bool> {
        Ok(self.store.list_documents().await?.is_empty())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_index_and_search_flow() {
        // Validate that the high-level API compiles and runs
        let store = VectorStore::open(std::env::temp_dir().join("rag-test-index-v2"))
            .await
            .expect("open store");

        let chunker = Chunker::new(100, 10).unwrap();
        let index = RagIndex::new(
            store,
            chunker,
            5,
            "openai".to_string(),
            "text-embedding-3-small".to_string(),
        )
        .await;

        let doc = Document {
            id: "test-doc".to_string(),
            content: "This is a test document for RAG indexing.".to_string(),
            metadata: std::collections::BTreeMap::new(),
        };

        // Without a config, index_document will fail due to no provider config
        // This is expected behavior for unit testing
        let default_cfg = AppConfig::default();
        let result = index.index_document(doc, &default_cfg).await;
        assert!(result.is_err()); // Expected: no such provider
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("rag-test-index-v2"));
    }
}
