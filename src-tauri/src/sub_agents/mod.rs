//! Sub-Agent Runtime — async task execution for the main agent.
//!
//! This module provides infrastructure for the main agent to spawn sub-agents
//! that run **asynchronously** via `tokio::spawn`. The main agent can continue
//! its turn and check results later, enabling concurrent work patterns.
//!
//! ## Architecture
//!
//! ```text
//! Main Agent Turn
//!   ├─ Calls sub_agent tool → returns immediately with agent_id
//!   ├─ Main agent continues its turn loop
//!   ├─ Later: sub_agent(action='status', agentId=...) → gets progress
//!   └─ Optional: sub_agent(action='cancel', agentId=...) → stops the sub-agent
//!
//! Sub-Agent (runs in tokio::spawn)
//!   ├─ In-memory context (no disk I/O, auto-cleaned on drop)
//!   ├─ Calls AgentRuntime::run_turn (reuses full engine infrastructure)
//!   ├─ Streams progress events → frontend via chat_stream_event channel
//!   └─ Stores result in SubAgentTask state, notifies waiters
//! ```
//!
//! ## Key Design Decisions
//!
//! - **Process-wide static registry** (mirrors `background_jobs.rs`) — sub-agents
//!   outlive individual Tauri command invocations.
//! - **Reuse `AgentRuntime::run_turn`** — sub-agents get full multi-hop tool-using
//!   capability for free.
//! - **In-memory context** — ephemeral sub-agents don't need SQLite or LCM;
//!   a lightweight `Vec<StoredMessage>` suffices.
//! - **Scope-narrowing invariant** (from `sub_agent_tools.rs`) — non-root sub-agents
//!   must declare `delegated_scope` and `kept_work` to prevent infinite delegation
//!   chains.

pub(crate) mod events;
pub(crate) mod manager;
pub(crate) mod runner;
pub(crate) mod types;
mod worktree;

pub use events::SubAgentEvent;
