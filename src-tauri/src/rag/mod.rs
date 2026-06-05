//! RAG (Retrieval-Augmented Generation) module.
//!
//! Provides document indexing, chunking, embedding, and vector search
//! backed by LanceDB. The main entry point is [`index::RagIndex`].
//!
//! ## Module layout
//!
//! - [`types`] — Document, Chunk, SearchResult, SearchConfig
//! - [`chunking`] — Fixed-size text splitting with configurable overlap
//! - [`vector_store`] — LanceDB-backed vector store (create, insert, search, delete)
//! - [`index`] — High-level [`RagIndex`] that coordinates chunking, embedding, and search
//!
//! ## Usage
//!
//! ```ignore
//! use rag::{RagIndex, types::Document};
//!
//! let index = RagIndex::from_config(&app_config.rag).await?;
//!
//! // Index a document
//! index.index_document(Document { id: "doc-1".into(), content: "...", metadata: map }).await?;
//!
//! // Search
//! let results = index.search("my query", None).await?;
//! ```

pub(crate) mod chunking;
pub(crate) mod index;
pub(crate) mod types;
pub(crate) mod vector_store;

#[allow(unused_imports)]
pub use index::RagIndex;
#[allow(unused_imports)]
pub use types::{Chunk, Document, SearchConfig, SearchResult};
