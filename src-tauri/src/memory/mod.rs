//! Async Memory — persistent cross-conversation memory storage.
//!
//! The memory system stores Markdown files with YAML frontmatter in
//! `~/.agentjax/memory/`. Each file represents one "memory" with a name,
//! description, type, tags, and body content. An auto-generated `MEMORY.md`
//! index file provides quick context loading.
//!
//! ## File Format
//!
//! ```markdown
//! ---
//! name: project-architecture
//! description: Overview of AgentJax architecture
//! type: reference
//! tags: [architecture, rust]
//! ---
//!
//! Content body with **markdown**. Wiki-style [[links]] to other memories.
//! ```

mod context;
mod index;
pub(crate) mod search;
pub(crate) mod store;
pub(crate) mod types;

pub use context::build_memory_context;
pub use index::MemoryIndex;
pub use search::search_memories;
pub use store::MemoryStore;
pub use types::{MemoryFrontmatter, MemoryIndexEntry, MemoryType, ParsedMemory};
