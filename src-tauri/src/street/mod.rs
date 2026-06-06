//! Street — Unified async notification queue for proactive context injection.
//!
//! The Street collects results from async work (sub-agents, background tools,
//! memory agent) and delivers them to the main agent at the start of the next
//! turn, eliminating the need for the model to poll for status.
//!
//! ## Architecture
//!
//! ```text
//! Async completion sites → StreetManager::deposit() → in-memory queue + persist to JSONL
//!                                                          │
//! Next turn start → collect_pending() → user-role item → context injection
//!                                                          │
//!                               StreetEvent → mpsc channel → frontend (badge/auto-trigger)
//!                                                          │
//! App restart → load_items() from JSONL → rebuild in-memory queue
//! ```

pub(crate) mod context;
pub(crate) mod manager;
pub(crate) mod persist;
pub(crate) mod types;

pub use context::{build_street_context_item, format_street_items};
pub use manager::StreetManager;
pub use persist::notification_path;
pub use types::{Priority, StreetEvent, StreetItem, StreetSnapshot, StreetSource};
