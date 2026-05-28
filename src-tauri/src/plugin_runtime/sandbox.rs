use serde::{Deserialize, Serialize};

/// Sandbox policy for plugin code and future deno_core-backed script hosts.
///
/// The defaults intentionally start narrow so plugin execution only gains
/// access to capabilities that the host explicitly opts into later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, rename_all = "camelCase")]
pub struct SandboxPolicy {
    #[serde(default)]
    pub allow_file_read: bool,
    #[serde(default)]
    pub allow_file_write: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub allow_process_spawn: bool,
    #[serde(default)]
    pub allow_env_read: bool,
    pub max_memory_mb: Option<u64>,
    pub max_execution_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_hosts: Vec<String>,
}

impl Default for SandboxPolicy {
    fn default() -> Self {
        Self {
            allow_file_read: false,
            allow_file_write: false,
            allow_network: false,
            allow_process_spawn: false,
            allow_env_read: false,
            max_memory_mb: None,
            max_execution_ms: None,
            allowed_hosts: Vec::new(),
        }
    }
}
