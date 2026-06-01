//! LCM (Lossless Context Management) module.
//!
//! Implements the deterministic, engine-driven context management architecture
//! described in "LCM: Lossless Context Management" (Ehrlich & Blackman, 2026).
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────┐
//! │                  LcmEngine                       │
//! │  ┌──────────────┐  ┌────────────┐  ┌──────────┐ │
//! │  │ Immutable    │  │ Summary    │  │ Active    │ │
//! │  │ Store (SQLite)│  │ DAG        │  │ Context   │ │
//! │  │              │  │            │  │ Assembler │ │
//! │  │ messages     │  │ summaries  │  │           │ │
//! │  │ file_refs    │  │ edges      │  │ Raw +     │ │
//! │  │ fts index    │  │            │  │ Summary   │ │
//! │  └──────────────┘  └────────────┘  │ Pointers  │ │
//! │                                     └──────────┘ │
//! └─────────────────────────────────────────────────┘
//! ```
//!
//! ## Key Invariants
//!
//! 1. **Lossless**: Every message's original content is permanently retained
//!    in the immutable store, reachable via `lcm_grep` or `lcm_expand`.
//! 2. **Deterministic**: Context compaction is engine-driven using fixed
//!    three-level escalation, never delegated to the model.
//! 3. **Zero-Cost Continuity**: Below τ_soft, the store acts as a passive
//!    logger with no overhead.
//! 4. **DAG-structured summaries**: Summary nodes form a directed acyclic
//!    graph, allowing multi-resolution traversal of conversation history.

pub mod compaction;
pub mod dag;
pub mod engine;
pub mod file_handler;
pub mod store;
pub mod types;

pub use compaction::{CompactionEngine, NoopSummarizer, Summarizer};
pub use dag::SummaryDag;
pub use engine::LcmEngine;
pub use file_handler::FileHandler;
pub use store::LcmStore;
pub use types::*;
