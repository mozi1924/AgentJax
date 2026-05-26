mod tool_archiving;
mod tool_parsing;
mod turn;

#[cfg(test)]
mod tests;

pub struct AgentRuntime;

const MAX_TOOL_EXEC_RETRIES: usize = 2;
const MAX_REPEATED_FAILED_SIGNATURES: usize = 3;
