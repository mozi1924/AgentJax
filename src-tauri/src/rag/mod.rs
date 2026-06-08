//! RAG (Retrieval-Augmented Generation) module.
//!
//! Provides document indexing, chunking, embedding, and hybrid (keyword +
//! vector) search backed by LanceDB and SQLite FTS5.
//!
//! ## Module layout
//!
//! - [`types`] — Core RAG types: Document, Chunk, SearchResult, SearchConfig
//! - [`engine`] — `RagEngine`: unified embedding + hybrid search entry point
//! - [`chunking`] — Markdown-aware text splitting with distance-decay scoring
//! - [`vector_store`] — LanceDB-backed vector store
//! - [`fts_store`] — SQLite FTS5 keyword search store
//! - [`knowledge_base`] — Global `KnowledgeBaseManager` with hybrid search
//!
//! ## Usage
//!
//! ```ignore
//! use rag::knowledge_base::KnowledgeBaseManager;
//!
//! let kb_manager = KnowledgeBaseManager::from_config(&app_config)?;
//! let results = kb_manager.search("my_kb", "query text", 10, &app_config).await?;
//! ```

pub(crate) mod chunking;
pub(crate) mod engine;
pub(crate) mod file_watcher;
pub(crate) mod fts_store;
pub(crate) mod knowledge_base;
pub mod types;
pub(crate) mod vector_store;

#[allow(unused_imports)]
pub(crate) use engine::RagEngine;
#[allow(unused_imports)]
pub(crate) use fts_store::FtsStore;
#[allow(unused_imports)]
pub use knowledge_base::KnowledgeBaseManager;
#[allow(unused_imports)]
pub use types::{Chunk, Document, SearchConfig};
