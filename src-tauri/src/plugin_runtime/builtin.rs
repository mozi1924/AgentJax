use super::{
    PLUGIN_API_VERSION, PluginManifest, PluginPackage, PluginProviderDefinition, SandboxPolicy,
};
use serde_json::json;
use std::collections::BTreeMap;
use std::path::PathBuf;

const BUILTIN_ENTRYPOINT: &str = "globalThis.AgentJaxPlugin = { tools: {} };";

/// Return plugin packages compiled into the AgentJax binary.
///
/// Built-in plugins use the same manifest shape as user plugins. Keeping this
/// path as `PluginPackage` data lets the rest of the runtime treat bundled and
/// home-directory plugins uniformly while avoiding filesystem dependency for
/// built-in provider declarations.
pub fn builtin_plugin_packages() -> Vec<PluginPackage> {
    vec![agentjax_provider_package()]
}

fn agentjax_provider_package() -> PluginPackage {
    let manifest = PluginManifest {
        id: "agentjax.providers".to_string(),
        name: "AgentJax Model Providers".to_string(),
        version: env!("CARGO_PKG_VERSION").to_string(),
        api_version: PLUGIN_API_VERSION,
        entrypoint: "plugin.js".to_string(),
        description: "Built-in model provider plugins shipped with AgentJax.".to_string(),
        tools: Vec::new(),
        settings_sections: Vec::new(),
        settings_data: BTreeMap::new(),
        providers: vec![
            PluginProviderDefinition {
                kind: "openai-responses".to_string(),
                display_name: "OpenAI Responses".to_string(),
                transport_family: Some("responses".to_string()),
                default_api_endpoint: "https://api.openai.com/v1".to_string(),
                default_realtime_endpoint: None,
                default_credential_env: "OPENAI_API_KEY".to_string(),
                default_supports_websockets: true,
                default_stream_transport: "websocket".to_string(),
                default_model_ids: vec!["gpt-5-mini".to_string(), "gpt-5".to_string()],
                capabilities: Some(json!({
                    "requiresInstructions": true,
                    "requiresStreamTrueInWebsocket": true,
                    "supportsStoredResponses": false,
                    "supportsCrossSocketContinuation": false,
                    "supportsGenerateFalse": true,
                    "supportsJsonMode": true,
                    "supportsJsonSchema": true,
                    "supportsParallelToolCalls": true,
                    "supportsBuiltInWebSearch": false,
                    "emitsFinalOutputItems": true,
                    "emitsIncrementalToolCallArguments": true
                })),
                tool_schema_format: Some("responses".to_string()),
                ..Default::default()
            },
            PluginProviderDefinition {
                kind: "chat-completions".to_string(),
                display_name: "Chat Completions".to_string(),
                transport_family: Some("chat_completions".to_string()),
                default_api_endpoint: "https://api.openai.com/v1".to_string(),
                default_realtime_endpoint: None,
                default_credential_env: "OPENAI_API_KEY".to_string(),
                default_supports_websockets: false,
                default_stream_transport: "sse".to_string(),
                default_model_ids: vec!["gpt-4.1".to_string(), "gpt-4o".to_string()],
                capabilities: Some(json!({
                    "requiresInstructions": false,
                    "requiresStreamTrueInWebsocket": false,
                    "supportsStoredResponses": false,
                    "supportsCrossSocketContinuation": false,
                    "supportsGenerateFalse": false,
                    "supportsJsonMode": true,
                    "supportsJsonSchema": true,
                    "supportsParallelToolCalls": true,
                    "supportsBuiltInWebSearch": false,
                    "emitsFinalOutputItems": false,
                    "emitsIncrementalToolCallArguments": true
                })),
                tool_schema_format: Some("chat_completions".to_string()),
                ..Default::default()
            },
            PluginProviderDefinition {
                kind: "gemini".to_string(),
                display_name: "Gemini".to_string(),
                transport_family: Some("gemini".to_string()),
                default_api_endpoint: "https://generativelanguage.googleapis.com/v1beta"
                    .to_string(),
                default_realtime_endpoint: None,
                default_credential_env: "GEMINI_API_KEY".to_string(),
                default_supports_websockets: false,
                default_stream_transport: "sse".to_string(),
                default_model_ids: vec![
                    "gemini-2.5-flash".to_string(),
                    "gemini-2.5-pro".to_string(),
                ],
                capabilities: Some(json!({
                    "requiresInstructions": false,
                    "requiresStreamTrueInWebsocket": false,
                    "supportsStoredResponses": false,
                    "supportsCrossSocketContinuation": false,
                    "supportsGenerateFalse": false,
                    "supportsJsonMode": true,
                    "supportsJsonSchema": true,
                    "supportsParallelToolCalls": true,
                    "supportsBuiltInWebSearch": false,
                    "emitsFinalOutputItems": false,
                    "emitsIncrementalToolCallArguments": false
                })),
                tool_schema_format: Some("gemini".to_string()),
                ..Default::default()
            },
            PluginProviderDefinition {
                kind: "anthropic".to_string(),
                display_name: "Anthropic".to_string(),
                transport_family: Some("anthropic".to_string()),
                default_api_endpoint: "https://api.anthropic.com/v1".to_string(),
                default_realtime_endpoint: None,
                default_credential_env: "ANTHROPIC_API_KEY".to_string(),
                default_supports_websockets: false,
                default_stream_transport: "sse".to_string(),
                default_model_ids: vec![
                    "claude-sonnet-4-5".to_string(),
                    "claude-opus-4-1".to_string(),
                ],
                capabilities: Some(json!({
                    "requiresInstructions": false,
                    "requiresStreamTrueInWebsocket": false,
                    "supportsStoredResponses": false,
                    "supportsCrossSocketContinuation": false,
                    "supportsGenerateFalse": false,
                    "supportsJsonMode": false,
                    "supportsJsonSchema": false,
                    "supportsParallelToolCalls": true,
                    "supportsBuiltInWebSearch": false,
                    "emitsFinalOutputItems": false,
                    "emitsIncrementalToolCallArguments": true
                })),
                tool_schema_format: Some("anthropic".to_string()),
                ..Default::default()
            },
        ],
        sandbox: SandboxPolicy::default(),
    };

    manifest
        .validate()
        .expect("built-in provider plugin manifest must be valid");

    PluginPackage {
        manifest,
        root_dir: PathBuf::from("<agentjax-builtin>/providers"),
        manifest_path: PathBuf::from("<agentjax-builtin>/providers/plugin.json"),
        entrypoint_source: Some(BUILTIN_ENTRYPOINT.to_string()),
    }
}
