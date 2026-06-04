mod app_config;
mod constants;
mod default_items;
mod dynamic_options;
mod io;
mod model_ref;
mod prompt_composer;
mod schema;
mod path_validator;
mod settings;
mod settings_ui;

pub use io::{
    ConfigInfo, ConfigUpgradeResult, config_dir_path, get_config_info, init_config_if_missing,
    load_config, serialize_config_to_yaml, upgrade_config_file,
};
#[allow(unused_imports)]
pub use prompt_composer::{
    CompiledPromptAssembly, PromptBlock, PromptBlockRole, PromptBlockSource, PromptComposerConfig,
    compile_prompt_composer, normalize_prompt_composer,
};
#[allow(unused_imports)]
pub use schema::{
    AppConfig, ContextManagementConfig, McpConfig, McpRuntimeConfig, McpServerConfig,
    McpStdioRuntimeConfig, McpToolSourcePolicyConfig, McpTransportKind, MemoryConfig,
    ModelRequestConfig, PluginEntryConfig, PluginManagerConfig, PluginPermissionOverride,
    ProviderConfig, ProviderModelConfig, RagConfig, EmbeddingProviderConfig, ResolvedModelConfig,
    SubAgentConfig, ToolEnabledConfig, ToolManagerConfig, ToolSourcePolicyConfig,
};
#[allow(unused_imports)]
pub use settings::{
    SecretStatus, SettingsOption, SettingsPatch, SettingsPatchOperation, SettingsSnapshot,
    apply_settings_patch, get_settings_snapshot, get_settings_ui_snapshot,
};
#[allow(unused_imports)]
pub use dynamic_options::build_dynamic_options;
pub use settings_ui::SettingsUiSnapshot;

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static tokio::sync::Mutex<()> {
    use std::sync::OnceLock;

    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::constants::{BUILTIN_CORE_SYSTEM_BLOCK_ID, BUILTIN_CORE_SYSTEM_TITLE};
    use std::fs;

    /// Helper: build a minimal AppConfig with an openai provider and two models.
    fn test_config_with_openai() -> AppConfig {
        let mut cfg = AppConfig::default();
        let mut custom_settings = serde_json::Map::new();
        custom_settings.insert(
            "apiEndpoint".to_string(),
            serde_json::Value::String("https://api.openai.com/v1".to_string()),
        );
        let mut models = std::collections::BTreeMap::new();
        models.insert(
            "gpt-5".to_string(),
            ProviderModelConfig { enabled: true, ..Default::default() },
        );
        models.insert(
            "gpt-5-mini".to_string(),
            ProviderModelConfig { enabled: true, ..Default::default() },
        );
        cfg.providers.insert(
            "openai".to_string(),
            ProviderConfig {
                kind: "openai".to_string(),
                models,
                custom_settings,
            },
        );
        cfg.active_provider = "openai".to_string();
        cfg.default_model = "openai/gpt-5-mini".to_string();
        cfg.utility_small_model = "openai/gpt-5-mini".to_string();
        cfg
    }

    #[test]
    fn resolves_model_with_provider_scoped_reference() {
        let cfg = test_config_with_openai().normalize();
        let resolved = cfg
            .resolve_model_profile(Some("openai/gpt-5"))
            .expect("resolve model");
        assert_eq!(resolved.provider_key, "openai");
        assert_eq!(resolved.model_id, "gpt-5");
        assert_eq!(resolved.model_ref, "openai/gpt-5");
    }

    #[test]
    fn falls_back_to_default_when_requested_model_invalid() {
        let cfg = test_config_with_openai().normalize();
        let resolved = cfg
            .resolve_model_profile(Some("openai/not-exist"))
            .expect("fallback to default");
        assert_eq!(resolved.model_ref, cfg.default_model);
    }

    #[test]
    fn keeps_unresolved_model_refs_during_normalize() {
        let cfg = AppConfig {
            default_model: "cm/gpt-5.4-mini".to_string(),
            utility_small_model: "cm/gpt-5.4-mini".to_string(),
            ..Default::default()
        };

        let normalized = cfg.normalize();
        assert_eq!(normalized.default_model, "cm/gpt-5.4-mini");
        assert_eq!(normalized.utility_small_model, "cm/gpt-5.4-mini");
    }

    #[test]
    fn resolve_profile_falls_back_to_first_enabled_model_when_defaults_are_unresolved() {
        let cfg = test_config_with_openai();

        let normalized = cfg.normalize();
        let resolved = normalized
            .resolve_model_profile(Some("openai/nonexistent-model-xyz"))
            .expect("fallback to first enabled model");

        assert!(
            normalized
                .configured_models()
                .into_iter()
                .any(|entry| entry == resolved.model_ref)
        );
    }



    #[test]
    fn resolved_profile_uses_built_in_agent_prompt() {
        let cfg = test_config_with_openai().normalize();
        let resolved = cfg.resolve_model_profile(None).expect("resolve");
        assert!(resolved.system_prompt.contains("agentic coding assistant"));
        assert!(resolved.system_prompt.contains("Commentary protocol"));
        assert!(resolved.system_prompt.contains("Background tool protocol"));
    }

    #[test]
    fn resolved_profile_compiles_user_system_and_developer_blocks() {
        let mut cfg = test_config_with_openai();
        cfg.prompt_composer.blocks.push(PromptBlock {
            id: "user-system".to_string(),
            title: "User system".to_string(),
            role: PromptBlockRole::System,
            content: "Always prefer concise diffs.".to_string(),
            enabled: true,
            source: PromptBlockSource::User,
            source_id: None,
            locked: false,
        });
        cfg.prompt_composer.blocks.push(PromptBlock {
            id: "user-developer".to_string(),
            title: "User developer".to_string(),
            role: PromptBlockRole::Developer,
            content: "Before each tool phase, emit a short Chinese commentary.".to_string(),
            enabled: true,
            source: PromptBlockSource::User,
            source_id: None,
            locked: false,
        });
        let resolved = cfg
            .normalize()
            .resolve_model_profile(None)
            .expect("resolve");
        assert!(resolved.system_prompt.contains("agentic coding assistant"));
        assert!(
            resolved
                .system_prompt
                .contains("Always prefer concise diffs.")
        );
        assert_eq!(resolved.prompt_assembly.developer_items.len(), 1);
    }

    #[test]
    fn load_config_does_not_rewrite_file_on_startup() {
        let _guard = test_env_lock()
            .blocking_lock();
        let home =
            std::env::temp_dir().join(format!("agentjax-config-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create temp home");
        let path = home.join("config.yaml");
        let raw = [
            "active_provider: \"cm\"",
            "default_model: \"cm/gpt-5.4-mini\"",
            "utility_small_model: \"cm/gpt-5.4-mini\"",
            "request_timeout_seconds: 77",
            "prompt_composer:",
            "  blocks:",
            "    - id: builtin-core-system",
            "      enabled: true",
            "    - id: \"user-system\"",
            "      title: \"User system\"",
            "      role: \"system\"",
            "      content: \" custom prompt \"",
            "      enabled: true",
            "      source: \"user\"",
            "      source_id: null",
            "      locked: false",
            "providers:",
            "  cm:",
            "    kind: \"codex\"",
            "    apiEndpoint: \"https://example.com/v1\"",
            "    realtimeEndpoint: \"\"",
            "    streamTransport: \"websocket\"",
            "    credential: \"\"",
            "    credentialEnv: \"CM_API_KEY\"",
            "    requestTimeoutSeconds: 66",
            "    models:",
            "      profile_a:",
            "        enabled: true",
            "        request:",
            "          reasoning_effort: \"high\"",
            "          extra_body: {}",
            "mcp:",
            "  stdio:",
            "    inherit_parent_env: false",
            "    env: {}",
            "  startup_timeout_ms: 30000",
            "  tool_timeout_ms: 120000",
            "  servers: {}",
            "",
        ]
        .join("\n");
        fs::write(&path, &raw).expect("write config");

        unsafe {
            std::env::set_var(crate::agentjax_home::AGENTJAX_HOME_ENV, &home);
        }
        let _ = load_config().expect("load config");
        let after = fs::read_to_string(&path).expect("read config after load");

        assert_eq!(after, raw);

        unsafe {
            std::env::remove_var(crate::agentjax_home::AGENTJAX_HOME_ENV);
        }
        let _ = fs::remove_dir_all(home);
    }

    #[test]
    fn provider_normalize_forces_sse_when_websocket_not_supported() {
        let mut cfg = test_config_with_openai();
        let provider = cfg
            .providers
            .get_mut("openai")
            .expect("openai provider exists");
        provider.custom_settings.insert(
            "supportsWebsockets".to_string(),
            serde_json::Value::Bool(false),
        );
        provider.custom_settings.insert(
            "streamTransport".to_string(),
            serde_json::Value::String("websocket".to_string()),
        );

        let normalized = cfg.normalize();
        let provider = normalized
            .providers
            .get("openai")
            .expect("normalized provider exists");
        assert_eq!(provider.stream_transport(), "sse");
    }


    #[test]
    fn provider_resolved_http_headers_merges_env_values() {
        let _guard = test_env_lock()
            .blocking_lock();
        let mut provider = ProviderConfig::default();

        let mut http_headers = serde_json::Map::new();
        http_headers.insert(
            "X-Feature".to_string(),
            serde_json::Value::String("static".to_string()),
        );
        provider.custom_settings.insert(
            "httpHeaders".to_string(),
            serde_json::Value::Object(http_headers),
        );

        let mut env_http_headers = serde_json::Map::new();
        env_http_headers.insert(
            "Authorization".to_string(),
            serde_json::Value::String("TEST_AGENTJAX_AUTH".to_string()),
        );
        env_http_headers.insert(
            "X-Feature".to_string(),
            serde_json::Value::String("TEST_AGENTJAX_X_FEATURE".to_string()),
        );
        provider.custom_settings.insert(
            "envHttpHeaders".to_string(),
            serde_json::Value::Object(env_http_headers),
        );

        unsafe {
            std::env::set_var("TEST_AGENTJAX_AUTH", "Bearer token-from-env");
            std::env::set_var("TEST_AGENTJAX_X_FEATURE", "env-value");
        }

        let headers = provider.resolved_http_headers();
        assert_eq!(
            headers.get("Authorization").map(String::as_str),
            Some("Bearer token-from-env")
        );
        assert_eq!(
            headers.get("X-Feature").map(String::as_str),
            Some("env-value")
        );

        unsafe {
            std::env::remove_var("TEST_AGENTJAX_AUTH");
            std::env::remove_var("TEST_AGENTJAX_X_FEATURE");
        }
    }

    #[test]
    fn mcp_server_normalize_clears_stdio_fields_for_streamable_http() {
        let mut cfg = AppConfig::default();
        cfg.mcp.servers.insert(
            "remote-demo".to_string(),
            McpServerConfig {
                transport: McpTransportKind::StreamableHttp,
                command: "npx".to_string(),
                args: vec!["-y".to_string(), "demo".to_string()],
                env: [("A".to_string(), "B".to_string())].into_iter().collect(),
                cwd: Some("/tmp/demo".to_string()),
                use_global_stdio_env: false,
                inherit_parent_env: Some(true),
                uri: Some("https://example.com/mcp".to_string()),
                ..McpServerConfig::default()
            },
        );

        let normalized = cfg.normalize();
        let server = normalized
            .mcp.servers
            .get("remote-demo")
            .expect("normalized mcp server exists");

        assert_eq!(server.command, "");
        assert!(server.args.is_empty());
        assert!(server.env.is_empty());
        assert_eq!(server.cwd, None);
        assert!(server.use_global_stdio_env);
        assert_eq!(server.inherit_parent_env, None);
    }

    #[test]
    fn mcp_server_normalize_clears_streamable_http_fields_for_stdio() {
        let mut cfg = AppConfig::default();
        cfg.mcp.servers.insert(
            "local-demo".to_string(),
            McpServerConfig {
                transport: McpTransportKind::Stdio,
                command: "node".to_string(),
                args: vec!["server.js".to_string()],
                uri: Some("https://example.com/mcp".to_string()),
                auth_header: Some("Bearer token".to_string()),
                headers: [("X-Test".to_string(), "1".to_string())]
                    .into_iter()
                    .collect(),
                allow_stateless: false,
                channel_buffer_capacity: Some(256),
                reinit_on_expired_session: false,
                ..McpServerConfig::default()
            },
        );

        let normalized = cfg.normalize();
        let server = normalized
            .mcp.servers
            .get("local-demo")
            .expect("normalized mcp server exists");

        assert_eq!(server.uri, None);
        assert_eq!(server.auth_header, None);
        assert!(server.headers.is_empty());
        assert!(server.allow_stateless);
        assert_eq!(server.channel_buffer_capacity, None);
        assert!(server.reinit_on_expired_session);
    }

    #[test]
    fn normalize_prompt_composer_reinserts_builtin_block_and_keeps_system_first() {
        let normalized = normalize_prompt_composer(PromptComposerConfig {
            blocks: vec![PromptBlock {
                id: "custom-dev".to_string(),
                title: "Custom developer".to_string(),
                role: PromptBlockRole::Developer,
                content: "Do the thing".to_string(),
                enabled: true,
                source: PromptBlockSource::User,
                source_id: None,
                locked: false,
            }],
        });

        assert_eq!(
            normalized.blocks.first().map(|block| block.id.as_str()),
            Some(BUILTIN_CORE_SYSTEM_BLOCK_ID)
        );
        assert_eq!(
            normalized.blocks.first().map(|block| block.title.as_str()),
            Some(BUILTIN_CORE_SYSTEM_TITLE)
        );
        assert_eq!(
            normalized.blocks.last().map(|block| block.role),
            Some(PromptBlockRole::Developer)
        );
    }

    #[test]
    fn compile_prompt_composer_skips_disabled_blocks_and_preserves_developer_order() {
        let composer = PromptComposerConfig {
            blocks: vec![
                PromptBlock {
                    id: "sys-a".to_string(),
                    title: "Sys A".to_string(),
                    role: PromptBlockRole::System,
                    content: "A".to_string(),
                    enabled: true,
                    source: PromptBlockSource::User,
                    source_id: None,
                    locked: false,
                },
                PromptBlock {
                    id: "sys-b".to_string(),
                    title: "Sys B".to_string(),
                    role: PromptBlockRole::System,
                    content: "B".to_string(),
                    enabled: false,
                    source: PromptBlockSource::User,
                    source_id: None,
                    locked: false,
                },
                PromptBlock {
                    id: "dev-a".to_string(),
                    title: "Dev A".to_string(),
                    role: PromptBlockRole::Developer,
                    content: "First".to_string(),
                    enabled: true,
                    source: PromptBlockSource::Plugin,
                    source_id: Some("plugin/a".to_string()),
                    locked: true,
                },
                PromptBlock {
                    id: "dev-b".to_string(),
                    title: "Dev B".to_string(),
                    role: PromptBlockRole::Developer,
                    content: "Second".to_string(),
                    enabled: true,
                    source: PromptBlockSource::User,
                    source_id: None,
                    locked: false,
                },
            ],
        };

        let compiled = compile_prompt_composer(&composer);
        assert_eq!(compiled.instructions_text, "A");
        assert_eq!(compiled.developer_items.len(), 2);
        assert_eq!(
            compiled.developer_items[0]["content"][0]["text"].as_str(),
            Some("First")
        );
        assert_eq!(
            compiled.developer_items[1]["content"][0]["text"].as_str(),
            Some("Second")
        );
        assert!(
            compiled
                .preview_markdown
                .contains("## System / instructions")
        );
        assert!(compiled.preview_markdown.contains("## Developer messages"));
    }

    #[test]
    fn test_plugin_provider_registration_and_config_self_healing() {
        use crate::plugin_runtime::PluginProviderDefinition;
        use crate::provider_api::registry::{register_plugin_provider, unregister_plugin_provider};

        let plugin_provider = PluginProviderDefinition {
            kind: "custom-oauth-llm".to_string(),
            display_name: "Custom OAuth LLM".to_string(),
            config_schema: serde_json::json!({
                "type": "object",
                "properties": {
                    "apiKey": {
                        "type": "string",
                        "default": "default-oauth-key"
                    },
                    "apiEndpoint": {
                        "type": "string",
                        "default": "https://api.custom-oauth.com/v1"
                    },
                    "customParam": {
                        "type": "string",
                        "default": "custom-value"
                    }
                }
            }),
            default_model_ids: vec!["custom-model-1".to_string(), "custom-model-2".to_string()],
            capabilities: Some(serde_json::json!(
                crate::provider_api::ProviderCapabilities::chat_completions()
            )),
            tool_schema_format: Some("chat_completions".to_string()),
            ..Default::default()
        };

        // Register the provider
        register_plugin_provider(plugin_provider);

        // Create an AppConfig with the provider and explicit model config
        let mut models = std::collections::BTreeMap::new();
        models.insert(
            "custom-model-1".to_string(),
            ProviderModelConfig { enabled: true, ..Default::default() },
        );
        let provider_cfg = ProviderConfig {
            kind: "custom-oauth-llm".to_string(),
            models,
            ..Default::default()
        };
        let cfg = AppConfig {
            providers: [("custom-oauth-llm".to_string(), provider_cfg)]
                .into_iter()
                .collect(),
            ..Default::default()
        };

        // Normalize
        let normalized = cfg.normalize();
        let provider = normalized
            .providers
            .get("custom-oauth-llm")
            .expect("custom-oauth-llm provider exists");

        // Verify custom_settings has been auto-completed with defaults from the schema
        assert_eq!(
            provider
                .custom_settings
                .get("apiKey")
                .and_then(|v| v.as_str()),
            Some("default-oauth-key")
        );
        assert_eq!(
            provider
                .custom_settings
                .get("apiEndpoint")
                .and_then(|v| v.as_str()),
            Some("https://api.custom-oauth.com/v1")
        );
        assert_eq!(
            provider
                .custom_settings
                .get("customParam")
                .and_then(|v| v.as_str()),
            Some("custom-value")
        );

        // Verify defaults models are populated and enabled
        assert!(provider.models.contains_key("custom-model-1"));
        assert!(provider.models.get("custom-model-1").unwrap().enabled);

        // Clean up
        unregister_plugin_provider("custom-oauth-llm");
    }

    #[test]
    fn test_config_yaml_serialization_order_and_abbreviation() {
        let cfg = AppConfig {
            language: "zh-CN".to_string(),
            active_provider: "custom".to_string(),
            ..Default::default()
        };
        
        let yaml = serialize_config_to_yaml(&cfg).expect("serialize config");
        
        let language_idx = yaml.find("language:").expect("find language");
        let active_provider_idx = yaml.find("active_provider:").expect("find active_provider");
        let providers_idx = yaml.find("providers:").expect("find providers");
        let prompt_composer_idx = yaml.find("prompt_composer:").expect("find prompt_composer");
        
        assert!(language_idx < active_provider_idx);
        assert!(active_provider_idx < providers_idx);
        assert!(providers_idx < prompt_composer_idx);

        assert!(!yaml.contains("Commentary protocol"));
    }
}
