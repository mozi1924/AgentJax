pub const CONFIG_FILE_NAME: &str = "config.yaml";
pub const DEFAULT_SYSTEM_PROMPT: &str =
    "You are Codex, a helpful AI assistant. Follow the user's instructions.";
pub const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
pub const DEFAULT_DEFAULT_MODEL_REF: &str = "openai/gpt-5-mini";
pub const DEFAULT_UTILITY_SMALL_MODEL_REF: &str = "openai/gpt-5-mini";

pub const fn default_true() -> bool {
    true
}

pub const fn default_mcp_startup_timeout_ms() -> u64 {
    15_000
}

pub const fn default_mcp_tool_timeout_ms() -> u64 {
    30_000
}
