//! Plugin runtime abstractions for AgentJax.
//!
//! This module is the first boundary around `deno_core` so we can evolve the
//! plugin system, sandbox policy, and agent tool-call orchestration without
//! wiring the concrete V8 runtime into the rest of the backend too early.

mod api;
mod builtin;
mod discovery;
mod manifest;
mod orchestration;
mod runtime;
mod sandbox;

pub use api::{
    PLUGIN_API_VERSION, PLUGIN_SOURCE_TYPE, PLUGIN_TOOL_NAME_PREFIX, PluginInvocationContext,
    PluginToolCall, PluginToolResult, RegisteredPluginTool, prefixed_plugin_tool_name,
    registered_tools_for_manifest,
};
pub use builtin::builtin_plugin_packages;
pub use discovery::{
    PLUGIN_MANIFEST_FILE, PluginPackage, discover_all_plugin_packages,
    discover_home_plugin_packages, discover_plugin_packages, load_plugin_package,
};
pub use manifest::{
    PluginManifest, PluginProviderDefinition, PluginToolDefinition, PluginToolKind,
};
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
            api_version: PLUGIN_API_VERSION,
            entrypoint: String::new(),
            description: String::new(),
            tools: Vec::new(),
            settings_sections: Vec::new(),
            settings_data: Default::default(),
            providers: Vec::new(),
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

    #[test]
    fn manifest_validation_rejects_unsupported_api_version() {
        let manifest = PluginManifest {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            version: "0.1.0".to_string(),
            api_version: PLUGIN_API_VERSION + 1,
            entrypoint: "plugin.ts".to_string(),
            description: String::new(),
            tools: Vec::new(),
            settings_sections: Vec::new(),
            settings_data: Default::default(),
            providers: Vec::new(),
            sandbox: SandboxPolicy::default(),
        };

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn manifest_validation_validates_custom_providers() {
        let manifest_ok = PluginManifest {
            id: "oauth-llm".to_string(),
            name: "OAuth LLM".to_string(),
            version: "1.0.0".to_string(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: "plugin.js".to_string(),
            providers: vec![PluginProviderDefinition {
                kind: "custom-oauth".to_string(),
                display_name: "OAuth Provider".to_string(),
                config_schema: serde_json::json!({
                    "type": "object"
                }),
                default_model_ids: vec!["llama-3".to_string()],
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(manifest_ok.validate().is_ok());

        let manifest_err = PluginManifest {
            id: "oauth-llm".to_string(),
            name: "OAuth LLM".to_string(),
            version: "1.0.0".to_string(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: "plugin.js".to_string(),
            providers: vec![PluginProviderDefinition {
                kind: "".to_string(),
                display_name: "OAuth Provider".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };
        assert!(manifest_err.validate().is_err());
    }

    #[test]
    fn registered_plugin_tools_get_provider_safe_names() {
        let manifest = PluginManifest {
            id: "acme.demo".to_string(),
            name: "Demo".to_string(),
            version: "0.1.0".to_string(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: "plugin.ts".to_string(),
            description: String::new(),
            tools: vec![PluginToolDefinition {
                name: "hello-world".to_string(),
                display_name: "Hello World".to_string(),
                description: "Greets the caller".to_string(),
                icon: Some("Sparkles".to_string()),
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {}
                }),
                kind: PluginToolKind::Function,
            }],
            settings_sections: Vec::new(),
            settings_data: Default::default(),
            providers: Vec::new(),
            sandbox: SandboxPolicy::default(),
        };

        let tools = registered_tools_for_manifest(&manifest);
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].prefixed_name(), "plugin__acme_demo__hello-world");
    }

    #[test]
    fn runtime_prepares_tool_calls_with_manifest_sandbox() {
        let sandbox = SandboxPolicy {
            max_execution_ms: Some(30_000),
            ..SandboxPolicy::default()
        };
        let manifest = PluginManifest {
            id: "demo".to_string(),
            name: "Demo".to_string(),
            version: "0.1.0".to_string(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: "plugin.ts".to_string(),
            description: String::new(),
            tools: vec![PluginToolDefinition {
                name: "echo".to_string(),
                display_name: "Echo".to_string(),
                description: "Echoes input".to_string(),
                icon: None,
                input_schema: serde_json::json!({
                    "type": "object",
                    "properties": {
                        "message": { "type": "string" }
                    }
                }),
                kind: PluginToolKind::Function,
            }],
            settings_sections: Vec::new(),
            settings_data: Default::default(),
            providers: Vec::new(),
            sandbox: sandbox.clone(),
        };
        let mut runtime = DenoCorePluginRuntime::new(
            deno_core::RuntimeOptions::default(),
            SandboxPolicy::default(),
        );

        runtime
            .register_manifest(manifest)
            .expect("register manifest");
        let call = runtime
            .prepare_tool_call(
                "demo",
                "echo",
                serde_json::json!({ "message": "hi" }),
                PluginInvocationContext {
                    conversation_id: Some("conversation-1".to_string()),
                },
            )
            .expect("prepare plugin tool call");

        assert_eq!(call.plugin_id, "demo");
        assert_eq!(call.tool_name, "echo");
        assert_eq!(call.sandbox, sandbox);
        assert_eq!(
            runtime.registered_tools()[0].prefixed_name(),
            "plugin__demo__echo"
        );
    }

    #[test]
    fn discovery_loads_plugin_package_from_directory() {
        let root =
            std::env::temp_dir().join(format!("agentjax-plugin-package-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create plugin root");
        std::fs::write(
            root.join("plugin.js"),
            r#"globalThis.AgentJaxPlugin = { tools: {} };"#,
        )
        .expect("write plugin entrypoint");
        std::fs::write(
            root.join(PLUGIN_MANIFEST_FILE),
            serde_json::json!({
                "id": "demo",
                "name": "Demo",
                "version": "0.1.0",
                "apiVersion": PLUGIN_API_VERSION,
                "entrypoint": "plugin.js",
                "tools": []
            })
            .to_string(),
        )
        .expect("write plugin manifest");

        let package = load_plugin_package(&root).expect("load plugin package");

        assert_eq!(package.manifest.id, "demo");
        assert_eq!(package.root_dir, root);
        std::fs::remove_dir_all(package.root_dir).ok();
    }

    #[test]
    fn manifest_accepts_declarative_settings_sections() {
        let manifest = PluginManifest {
            id: "demo.settings".to_string(),
            name: "Demo Settings".to_string(),
            version: "0.1.0".to_string(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: "plugin.ts".to_string(),
            description: String::new(),
            tools: Vec::new(),
            settings_sections: vec![serde_json::json!({
                "id": "plugin.demo.settings",
                "title": "Demo Settings",
                "icon": "Puzzle",
                "order": 900,
                "children": [{
                    "kind": "collapsible",
                    "id": "plugin.demo.settings.advanced",
                    "title": "Advanced",
                    "defaultExpanded": false,
                    "children": []
                }]
            })],
            settings_data: Default::default(),
            providers: Vec::new(),
            sandbox: SandboxPolicy::default(),
        };

        assert!(manifest.validate().is_ok());
    }

    #[test]
    fn manifest_rejects_invalid_settings_sections() {
        let manifest = PluginManifest {
            id: "demo.settings".to_string(),
            name: "Demo Settings".to_string(),
            version: "0.1.0".to_string(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: "plugin.ts".to_string(),
            description: String::new(),
            tools: Vec::new(),
            settings_sections: vec![serde_json::json!({
                "id": "plugin.demo.settings",
                "children": [{
                    "kind": "panel",
                    "id": "plugin.demo.settings"
                }]
            })],
            settings_data: Default::default(),
            providers: Vec::new(),
            sandbox: SandboxPolicy::default(),
        };

        assert!(manifest.validate().is_err());
    }

    #[test]
    fn deno_core_runtime_executes_sync_plugin_tool() {
        let root =
            std::env::temp_dir().join(format!("agentjax-plugin-runtime-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).expect("create plugin root");
        std::fs::write(
            root.join("plugin.js"),
            r#"
globalThis.AgentJaxPlugin = {
  tools: {
    echo(args, context) {
      return {
        message: args.message,
        conversationId: context.conversationId,
      };
    }
  }
};
"#,
        )
        .expect("write plugin entrypoint");
        std::fs::write(
            root.join(PLUGIN_MANIFEST_FILE),
            serde_json::json!({
                "id": "demo",
                "name": "Demo",
                "version": "0.1.0",
                "apiVersion": PLUGIN_API_VERSION,
                "entrypoint": "plugin.js",
                "tools": [{
                    "name": "echo",
                    "description": "Echoes a message",
                    "inputSchema": {
                        "type": "object",
                        "properties": {
                            "message": { "type": "string" }
                        }
                    }
                }],
                "settingsSections": [{
                    "id": "plugin.demo.settings",
                    "title": "Demo Settings",
                    "icon": "Puzzle",
                    "order": 900,
                    "children": []
                }],
                "settingsData": {
                    "items": [{
                        "id": "primary",
                        "name": "Primary item",
                        "description": "Rendered by the shared SchemaRenderer plugin provider."
                    }]
                },
                "sandbox": {
                    "maxExecutionMs": 30000
                }
            })
            .to_string(),
        )
        .expect("write plugin manifest");
        let package = load_plugin_package(&root).expect("load plugin package");
        assert_eq!(package.manifest.settings_sections.len(), 1);
        assert_eq!(package.manifest.settings_data["items"][0]["id"], "primary");
        let mut runtime = DenoCorePluginRuntime::new(
            deno_core::RuntimeOptions::default(),
            SandboxPolicy::default(),
        );
        runtime
            .register_package(package)
            .expect("register plugin package");
        let call = runtime
            .prepare_tool_call(
                "demo",
                "echo",
                serde_json::json!({ "message": "hello" }),
                PluginInvocationContext {
                    conversation_id: Some("conversation-1".to_string()),
                },
            )
            .expect("prepare plugin call");

        let result = runtime
            .execute_tool_call(call)
            .expect("execute plugin tool");

        assert!(result.ok);
        assert_eq!(result.output["message"], "hello");
        assert_eq!(result.output["conversationId"], "conversation-1");
        std::fs::remove_dir_all(root).ok();
    }
}
