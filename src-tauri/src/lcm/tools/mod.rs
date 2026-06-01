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
//!   - `agentic_map` — parallel sub-agent sessions over a JSONL file
//!
//! - Delegation (§3.2):
//!   - `task` — delegate to a sub-agent with scope-narrowing invariant

pub mod agentic_map;
pub mod describe;
pub mod expand;
pub mod grep;
pub mod llm_map;
pub mod task;

pub use agentic_map::AgenticMapTool;
pub use describe::LcmDescribeTool;
pub use expand::LcmExpandTool;
pub use grep::LcmGrepTool;
pub use llm_map::LlmMapTool;
pub use task::TaskTool;
