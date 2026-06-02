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
//!   ├─ Calls spawn_sub_agent tool → returns immediately with agent_id
//!   ├─ Main agent continues its turn loop
//!   ├─ Later: sub_agent_status(agent_id) → gets progress/results
//!   └─ Optional: cancel_sub_agent(agent_id) → stops the sub-agent
//!
//! Sub-Agent (runs in tokio::spawn)
//!   ├─ Isolated LCM store at {parent_conv}/sub_agents/{agent_id}/lcm.db
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
//! - **Scope-narrowing invariant** (from `lcm/tools/task.rs`) — non-root sub-agents
//!   must declare `delegated_scope` and `kept_work` to prevent infinite delegation
//!   chains.

pub(crate) mod events;
mod lcm_context;
pub(crate) mod manager;
pub(crate) mod runner;
pub(crate) mod types;
mod worktree;

pub use events::SubAgentEvent;
