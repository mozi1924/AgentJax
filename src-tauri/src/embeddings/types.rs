//! Core types for the Embedding API.
//!
//! These are the input/output types used by all embedding providers.
//! The trait-bound request wraps into a provider-specific HTTP payload.

use serde::{Deserialize, Serialize};

/// A single embedding vector.
pub type Embedding = Vec<f32>;

/// Request to embed one or more text inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingRequest {
    /// Text strings to embed. The provider may restrict batch size.
    pub input: Vec<String>,
    /// Optional model override. When `None`, the provider's default is used.
    pub model: Option<String>,
    /// Dimensions to truncate to. When `None`, the provider's native dimension is used.
    pub dimensions: Option<usize>,
}

impl EmbeddingRequest {
    /// Create a request for a single text string.
    pub fn single(text: impl Into<String>) -> Self {
        Self {
            input: vec![text.into()],
            model: None,
            dimensions: None,
        }
    }

    /// Create a request from a batch of texts.
    pub fn batch(texts: Vec<String>) -> Self {
        Self {
            input: texts,
            model: None,
            dimensions: None,
        }
    }

    /// Set the model for this request.
    pub fn with_model(mut self, model: impl Into<String>) -> Self {
        self.model = Some(model.into());
        self
    }

    /// Set the output dimensions.
    pub fn with_dimensions(mut self, dims: usize) -> Self {
        self.dimensions = Some(dims);
        self
    }
}

/// Usage statistics for an embedding request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingUsage {
    /// Tokens consumed by the input prompt.
    pub prompt_tokens: Option<u32>,
    /// Total tokens consumed.
    pub total_tokens: Option<u32>,
}

/// The result of an embedding request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddingResponse {
    /// The embedding vectors, one per input text in the same order.
    pub embeddings: Vec<Embedding>,
    /// The model that produced the embeddings.
    pub model: String,
    /// Usage statistics, if available from the provider.
    #[serde(default)]
    pub usage: EmbeddingUsage,
}

impl EmbeddingResponse {
    /// Return the first embedding vector. Panics if empty.
    pub fn single(&self) -> &Embedding {
        &self.embeddings[0]
    }
}
