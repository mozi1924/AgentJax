//! Plugin runtime abstractions for AgentJax.
//!
//! This module is the first boundary around `deno_core` so we can evolve the
//! plugin system, sandbox policy, and agent tool-call orchestration without
//! wiring the concrete V8 runtime into the rest of the backend too early.

pub(crate) mod api;
mod builtin;
pub(crate) mod discovery;
mod hooks;
pub(crate) mod manifest;
mod orchestration;
mod runtime;
mod sandbox;
mod sdk;

pub use api::{
    PluginInvocationContext, PluginToolCall, PluginToolResult, RegisteredPluginTool,
    prefixed_plugin_tool_name, registered_tools_for_manifest,
};
pub use builtin::builtin_plugin_packages;
pub use discovery::{PluginPackage, discover_all_plugin_packages, discover_home_plugin_packages};
// AuthConfig/AuthPlacement re-exported for future phases (credential injection generalization).
#[allow(unused_imports)]
pub use manifest::{
    AuthConfig, AuthPlacement, BuiltinModelDescriptor, ModelRoutingRule, PluginManifest,
    PluginProviderDefinition, PluginToolDefinition, ReasoningSchema,
};
pub use runtime::{
    PluginRuntimeError, PluginRuntimeResult, create_temp_plugin_instance,
    provider_definitions_for_package,
};
pub use sandbox::SandboxPolicy;

#[cfg(test)]
mod tests {
    use super::api::PLUGIN_API_VERSION;
    use super::discovery::{PLUGIN_MANIFEST_FILE, load_plugin_package};
    use super::manifest::PluginToolKind;
    use super::orchestration::{ToolCallBatch, ToolCallRequest, ToolCallSource};
    use super::runtime::DenoCorePluginRuntime;
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
        let mut runtime = DenoCorePluginRuntime::new(SandboxPolicy::default());

        runtime
            .register_package(PluginPackage {
                manifest,
                root_dir: std::env::temp_dir(),
                manifest_path: std::env::temp_dir().join("plugin.json"),
                entrypoint_source: Some("globalThis.AgentJaxPlugin = { tools: { echo() { return {}; } }, providers: [] };".to_string()),
                is_builtin: false,
            })
            .expect("register package");
        let call = runtime
            .prepare_tool_call(
                "demo",
                "echo",
                serde_json::json!({ "message": "hi" }),
                PluginInvocationContext {
                    conversation_id: Some("conversation-1".to_string()),
                    model_id: None,
                    turn_id: None,
                    hop_index: None,
                    context_token_estimate: None,
                    message_count: None,
                    tool_call_count: None,
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
    fn builtin_provider_plugins_are_loaded_from_compiled_plugin_directories() {
        let packages = builtin_plugin_packages();
        let plugin_ids = packages
            .iter()
            .map(|package| package.manifest.id.as_str())
            .collect::<Vec<_>>();

        assert_eq!(
            packages.len(),
            2,
            "Expected 2 built-in provider plugins (openai, deepseek), got: {plugin_ids:?}"
        );
        assert!(
            plugin_ids.contains(&"agentjax.provider.openai"),
            "Expected openai plugin, got: {plugin_ids:?}"
        );
        assert!(
            plugin_ids.contains(&"agentjax.provider.deepseek"),
            "Expected deepseek plugin, got: {plugin_ids:?}"
        );

        for package in packages {
            let providers = provider_definitions_for_package(&package)
                .expect("built-in provider plugin should export provider definitions");
            assert!(
                package.root_dir.starts_with("src-tauri/builtin-plugins"),
                "built-in plugin should keep its visible source directory: {:?}",
                package.root_dir
            );
            assert_eq!(package.manifest.entrypoint, "plugin.js");
            assert!(
                providers[0]
                    .config_schema
                    .get("properties")
                    .and_then(serde_json::Value::as_object)
                    .is_some_and(|properties| properties.contains_key("apiEndpoint")
                        && properties.contains_key("credentialEnv")),
                "built-in provider plugin should declare its required config fields"
            );
            // Phase 2: provider definition is now in manifest (declarative JSON),
            // not in JS. The JS entrypoint still exists for backward compat but
            // the provider metadata comes from the manifest.
            assert_eq!(
                package.manifest.providers.len(),
                1,
                "provider definition should be in plugin.json"
            );
            assert_eq!(
                providers.len(),
                1,
                "provider_definitions_for_package should yield 1 provider"
            );
        }
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
        let mut runtime = DenoCorePluginRuntime::new(SandboxPolicy::default());
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
                    model_id: None,
                    turn_id: None,
                    hop_index: None,
                    context_token_estimate: None,
                    message_count: None,
                    tool_call_count: None,
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
