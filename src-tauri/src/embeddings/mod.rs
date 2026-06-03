//! Embedding API — a trait-based abstraction for text embedding providers.
//!
//! This module provides a clean [`EmbeddingProvider`] trait along with a static
//! registry and built-in providers (starting with OpenAI).
//!
//! ## Usage
//!
//! ```ignore
//! use crate::embeddings::registry;
//! use crate::embeddings::types::EmbeddingRequest;
//!
//! registry::init_builtin_providers();
//! let provider = registry::get("openai").unwrap();
//! let response = provider.embed(&EmbeddingRequest::single("Hello world")).await?;
//! ```

pub(crate) mod openai;
pub(crate) mod provider;
pub(crate) mod registry;
pub(crate) mod types;

pub use provider::EmbeddingProvider;
// pub use re-exported via types module
