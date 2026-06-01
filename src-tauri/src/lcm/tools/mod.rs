//! LCM context tools — the model-facing interface to the immutable store.
//!
//! These tools implement the Memory-Access Tools described in Appendix C.1
//! of the LCM paper:
//!
//! - `lcm_grep` — regex search across the full immutable message history
//! - `lcm_describe` — metadata retrieval for any LCM entity
//! - `lcm_expand` — expand summary nodes (restricted to sub-agents)

pub mod grep;
pub mod describe;
pub mod expand;

pub use grep::LcmGrepTool;
pub use describe::LcmDescribeTool;
pub use expand::LcmExpandTool;
