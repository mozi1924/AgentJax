//! Street — Unified async notification queue for proactive context injection.
//!
//! The Street collects results from async work (sub-agents, background tools,
//! memory agent) and delivers them to the main agent at the start of the next
//! turn, eliminating the need for the model to poll for status.
//!
//! ## Architecture
//!
//! ```text
//! Async completion sites → StreetManager::deposit() → in-memory queue
//!                                                          │
//! Next turn start → collect_pending() → developer msg → context injection
//!                                                          │
//!                               StreetEvent → mpsc channel → frontend (badge/auto-trigger)
//! ```

pub(crate) mod context;
pub(crate) mod manager;
pub(crate) mod types;

pub use context::{build_street_context_developer_item, format_street_items};
pub use manager::StreetManager;
pub use types::{
    Priority, StreetEvent, StreetItem, StreetItemStatus, StreetSnapshot, StreetSource,
};
