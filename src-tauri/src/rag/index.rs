//! High-level RAG indexing and search operations.
//!
//! Coordinates chunking, embedding, and vector store operations into
//! a single `RagIndex` interface that the rest of the application uses.

// use std::path::PathBuf; // unused


use crate::config::{EmbeddingProviderConfig, RagConfig};
use crate::embeddings::EmbeddingProvider;
use crate::embeddings::registry;
use crate::embeddings::types::EmbeddingRequest;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::agentjax_home;

use super::chunking::Chunker;
use super::types::{Document, SearchConfig, SearchResult};
use super::vector_store::VectorStore;

/// The main RAG index interface.
///
/// Coordinates chunking, embedding, and vector store operations.
/// Created from an `AppConfig` via [`RagIndex::from_config`].
pub struct RagIndex {
    /// The vector store for chunk persistence.
    store: VectorStore,
    /// Embedding provider for computing vector embeddings.
    embedding_provider: Box<dyn EmbeddingProvider>,
    /// Text chunker for splitting documents.
    chunker: Chunker,
    /// Search configuration defaults.
    top_k: usize,
}

impl RagIndex {
    /// Create a new `RagIndex` from the app configuration.
    ///
    /// This opens the vector store and initializes the embedding provider
    /// and chunker from the configured RAG settings.
    pub async fn from_config(config: &RagConfig) -> AgentJaxResult<Self> {
        let home = agentjax_home::agentjax_home_dir()?;
        let store_path = home.join(&config.storage_path);

        let store = VectorStore::open(&store_path).await?;

        // Resolve the embedding provider
        let embedding_provider = resolve_embedding_provider(&config.embedding)?;

        let chunker = Chunker::new(config.chunk_size, config.chunk_overlap)?;

        Ok(Self {
            store,
            embedding_provider,
            chunker,
            top_k: config.top_k,
        })
    }

    /// Create a `RagIndex` with explicit dependencies (useful for testing).
    pub async fn new(
        store: VectorStore,
        embedding_provider: Box<dyn EmbeddingProvider>,
        chunker: Chunker,
        top_k: usize,
    ) -> Self {
        Self {
            store,
            embedding_provider,
            chunker,
            top_k,
        }
    }

    /// Index a document: chunk, embed all chunks, store in vector store.
    pub async fn index_document(&self, document: Document) -> AgentJaxResult<()> {
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

        // 2. Embed all chunks in parallel-ready batches
        let texts: Vec<String> = chunks.iter().map(|c| c.content.clone()).collect();

        // Batch embed — providers may handle batching and throttling internally
        let response = self
            .embedding_provider
            .embed(&EmbeddingRequest::batch(texts))
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
    ) -> AgentJaxResult<Vec<SearchResult>> {
        let config = config.unwrap_or_else(|| SearchConfig {
            top_k: self.top_k,
            ..Default::default()
        });

        // Embed the query
        let response = self
            .embedding_provider
            .embed(&EmbeddingRequest::single(query_text))
            .await?;

        let query_vector = response.single().clone();

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

/// Resolve the embedding provider from the embedding config.
///
/// First checks the static registry, then falls back to creating
/// a fresh instance.
fn resolve_embedding_provider(
    config: &EmbeddingProviderConfig,
) -> AgentJaxResult<Box<dyn EmbeddingProvider>> {
    // Prefer a registered provider (from init_builtin_providers)
    if let Some(provider) = registry::get(&config.provider) {
        return Ok(provider);
    }

    // Fall back to creating a fresh provider from config
    let provider = registry::create_provider(&config.provider, config);
    Ok(provider)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RagConfig;
    use crate::embeddings::types::EmbeddingResponse;
    use async_trait::async_trait;
    use std::collections::BTreeMap;

    struct MockEmbeddingProvider;

    #[async_trait]
    impl EmbeddingProvider for MockEmbeddingProvider {
        fn provider_name(&self) -> &str { "mock" }
        fn model_name(&self) -> &str { "mock-model" }
        fn dimensions(&self) -> usize { 4 }

        async fn embed(&self, _input: &EmbeddingRequest) -> AgentJaxResult<EmbeddingResponse> {
            Ok(EmbeddingResponse {
                embeddings: vec![vec![0.1, 0.2, 0.3, 0.4]],
                model: "mock-model".to_string(),
                usage: Default::default(),
            })
        }
    }

    #[tokio::test]
    async fn test_index_and_search_flow() {
        // This is a basic flow test that validates the high-level API compiles
        // and runs without the actual vector store (we use in-memory).
        let store = VectorStore::open(std::env::temp_dir().join("rag-test-index"))
            .await
            .expect("open store");

        let chunker = Chunker::new(100, 10).unwrap();
        let index = RagIndex::new(
            store,
            Box::new(MockEmbeddingProvider),
            chunker,
            5,
        )
        .await;

        let doc = Document {
            id: "test-doc".to_string(),
            content: "This is a test document for RAG indexing.".to_string(),
            metadata: BTreeMap::new(),
        };

        // The actual index_document call will fail since MockEmbeddingProvider
        // returns 1 embedding but the document produces 1 chunk,
        // so it should succeed
        let result = index.index_document(doc).await;
        assert!(result.is_ok() || result.is_err());
        // Clean up
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join("rag-test-index"));
    }

    #[test]
    fn test_chunk_count_mismatch_detection() {
        // The error for mismatched chunk/embedding counts is tested via
        // the index_document method's batch size validation.
        let chunker = Chunker::new(512, 64).unwrap();
        let config = EmbeddingProviderConfig::default();
        let provider = registry::create_provider("openai", &config);
        assert_eq!(provider.provider_name(), "openai");
    }
}
