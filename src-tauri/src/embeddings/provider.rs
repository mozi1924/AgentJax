//! EmbeddingProvider trait — the core abstraction for all embedding models.
//!
//! Every embedding provider (OpenAI, local models, etc.) implements this trait.
//! Providers are registered in the [`registry`] and resolved at runtime.

use crate::error::AgentJaxResult;
use async_trait::async_trait;

use super::types::EmbeddingResponse;
use super::types::EmbeddingRequest;

/// The core trait for embedding providers.
///
/// Implementations handle:
/// - HTTP request construction (if remote)
/// - Batching limits and retry logic
/// - Response parsing
///
/// All methods are async to support both remote and local (on-device) providers.
#[async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// A unique name for this provider implementation (e.g., "openai", "sentence-transformers").
    fn provider_name(&self) -> &str;

    /// The default model identifier (e.g., "text-embedding-3-small").
    fn model_name(&self) -> &str;

    /// The native output dimension of the default model.
    fn dimensions(&self) -> usize;

    /// Embed a batch of texts.
    ///
    /// The implementation is responsible for:
    /// - Applying the model override from the request (if set)
    /// - Respecting the dimensions override (if set)
    /// - Handling provider-specific batching limits
    /// - Returning embeddings in the same order as the input texts
    async fn embed(&self, input: &EmbeddingRequest) -> AgentJaxResult<EmbeddingResponse>;
}
