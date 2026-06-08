//! Knowledge Base module — application layer on top of the RAG engine.
//!
//! Manages multiple named knowledge bases, each backed by a LanceDB vector
//! store and a SQLite FTS5 keyword store. Knowledge bases are global
//! (stored under `$AGENTJAX_HOME/knowledge_bases/{id}/`) and available to
//! all agent profiles by default. Profiles can individually disable a KB.
//!
//! ## Module layout
//!
//! - [`types`] — KB-specific types: `KnowledgeBaseInfo`, `HybridSearchResult`, `IndexingProgress`
//! - [`manager`] — `KnowledgeBaseManager`: KB lifecycle, indexing, hybrid search
//! - [`tools`] — Agent tools: `kb_list`, `kb_search`, `kb_get`, `kb_index`
//! - [`commands`] — Tauri IPC commands for settings UI
//! - [`preretrieval`] — Automatic KB search before each turn
//! - [`file_watcher`] — File system watcher for auto-sync
//!
//! ## Relationship to RAG
//!
//! The `rag` module provides the generic retrieval engine (chunking, embedding,
//! vector store, FTS store). The KB module is the application layer that routes
//! KB IDs to the right stores, manages KB lifecycle, and provides agent tools.

pub mod commands;
pub mod file_watcher;
pub mod indexing;
pub mod manager;
pub mod preretrieval;
pub mod tools;
pub mod types;

#[allow(unused_imports)]
pub use manager::KnowledgeBaseManager;
