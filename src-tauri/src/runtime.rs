pub(crate) mod agent_context;
mod engine;
mod stream_collection;
pub(crate) mod tool_archiving;
mod tool_execution;
mod tool_parsing;

#[cfg(test)]
mod tests;

pub struct AgentRuntime;

const MAX_TOOL_EXEC_RETRIES: usize = 2;
const MAX_REPEATED_FAILED_SIGNATURES: usize = 3;
