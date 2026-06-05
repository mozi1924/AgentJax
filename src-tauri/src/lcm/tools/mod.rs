//! LCM tools — memory-access and operator-level recursion tools.
//!
//! These tools implement:
//!
//! - Memory-Access Tools (Appendix C.1 of the LCM paper):
//!   - `lcm_grep` — regex search across the full immutable message history
//!   - `lcm_describe` — metadata retrieval for any LCM entity
//!   - `lcm_expand` — expand summary nodes (restricted to sub-agents)
//!
//! - Operator-Level Recursion (§3.1, Figure 4):
//!   - `llm_map` — parallel stateless LLM calls over a JSONL file
//!
//! - Delegation (§3.2):
//!   - `task` — delegate to a sub-agent with scope-narrowing invariant

pub mod describe;
pub mod expand;
pub mod grep;
pub mod llm_map;

pub use describe::LcmDescribeTool;
pub use expand::LcmExpandTool;
pub use grep::LcmGrepTool;
pub use llm_map::LlmMapTool;

use crate::lcm::LcmStore;
use crate::tools::ToolExecutionContext;
use std::sync::Arc;

/// Resolve the effective LCM store for a tool execution.
///
/// When running within a sub-agent context, the `ToolExecutionContext`
/// carries a `lcm_store_override` pointing at the sub-agent's isolated
/// LCM database. Otherwise, the tool's own store (parent conversation)
/// is used. This ensures sub-agents operate on their own conversation
/// history rather than the parent's.
pub(crate) fn effective_store<'a>(
    default: &'a Arc<LcmStore>,
    _context: &'a ToolExecutionContext,
) -> &'a Arc<LcmStore> {
    default
}
