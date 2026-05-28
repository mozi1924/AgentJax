//! Plugin runtime abstractions for AgentJax.
//!
//! This module is the first boundary around `deno_core` so we can evolve the
//! plugin system, sandbox policy, and agent tool-call orchestration without
//! wiring the concrete V8 runtime into the rest of the backend too early.

mod manifest;
mod orchestration;
mod runtime;
mod sandbox;

pub use manifest::{PluginManifest, PluginToolDefinition, PluginToolKind};
pub use orchestration::{
    ToolCallBatch, ToolCallExecutionPolicy, ToolCallOutcome, ToolCallRequest, ToolCallSource,
};
pub use runtime::{DenoCorePluginRuntime, PluginRuntime, PluginRuntimeError, PluginRuntimeResult};
pub use sandbox::SandboxPolicy;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn manifest_validation_rejects_missing_identity() {
        let manifest = PluginManifest {
            id: String::new(),
            name: String::new(),
            version: "0.1.0".to_string(),
            entrypoint: String::new(),
            description: String::new(),
            tools: Vec::new(),
            sandbox: SandboxPolicy::default(),
        };

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn single_tool_call_batch_uses_conservative_defaults() {
        let request = ToolCallRequest {
            call_id: "call_1".to_string(),
            tool_name: "get_system_time".to_string(),
            arguments: json!({}),
            source: ToolCallSource::Native,
            conversation_id: Some("conversation-1".to_string()),
            turn_id: Some("turn-1".to_string()),
            hop_index: Some(0),
            sandbox: None,
        };

        let batch = ToolCallBatch::single(request.clone());
        assert_eq!(batch.requests, vec![request]);
        assert!(!batch.policy.allow_parallel);
        assert_eq!(batch.policy.max_parallelism, 1);
    }
}
