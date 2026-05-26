mod app_config;
mod constants;
mod io;
mod model_ref;
mod schema;
mod settings;
mod settings_ui;

pub use io::{
    config_dir_path, get_config_info, init_config_if_missing, load_config, upgrade_config_file,
    ConfigInfo, ConfigUpgradeResult,
};
#[allow(unused_imports)]
pub use schema::{
    AppConfig, McpRuntimeConfig, McpServerConfig, McpTransportKind, ModelRequestConfig,
    ProviderConfig, ResolvedModelConfig,
};
#[allow(unused_imports)]
pub use settings::{
    apply_settings_patch, get_settings_snapshot, get_settings_ui_snapshot, SecretStatus,
    SettingsOption, SettingsPatch, SettingsPatchOperation, SettingsSnapshot,
};
#[allow(unused_imports)]
pub use settings_ui::{build_dynamic_options, build_settings_sections, SettingsUiSnapshot};

#[cfg(test)]
pub(crate) fn test_env_lock() -> &'static std::sync::Mutex<()> {
    use std::sync::{Mutex, OnceLock};

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn resolves_model_with_provider_scoped_reference() {
        let cfg = AppConfig::default().normalize();
        let resolved = cfg
            .resolve_model_profile(Some("openai-responses/gpt-5"))
            .expect("resolve model");
        assert_eq!(resolved.provider_key, "openai-responses");
        assert_eq!(resolved.model_id, "gpt-5");
        assert_eq!(resolved.model_ref, "openai-responses/gpt-5");
    }

    #[test]
    fn falls_back_to_default_when_requested_model_invalid() {
        let cfg = AppConfig::default().normalize();
        let resolved = cfg
            .resolve_model_profile(Some("openai-responses/not-exist"))
            .expect("fallback to default");
        assert_eq!(resolved.model_ref, cfg.default_model);
    }

    #[test]
    fn keeps_unresolved_model_refs_during_normalize() {
        let mut cfg = AppConfig::default();
        cfg.default_model = "cm/gpt-5.4".to_string();
        cfg.utility_small_model = "cm/gpt-5.4".to_string();

        let normalized = cfg.normalize();
        assert_eq!(normalized.default_model, "cm/gpt-5.4");
        assert_eq!(normalized.utility_small_model, "cm/gpt-5.4");
    }

    #[test]
    fn resolve_profile_falls_back_to_first_enabled_model_when_defaults_are_unresolved() {
        let mut cfg = AppConfig::default();
        cfg.default_model = "cm/gpt-5.4".to_string();
        cfg.utility_small_model = "cm/gpt-5.4".to_string();

        let normalized = cfg.normalize();
        let resolved = normalized
            .resolve_model_profile(None)
            .expect("fallback to first enabled model");

        assert!(normalized
            .configured_models()
            .into_iter()
            .any(|entry| entry == resolved.model_ref));
    }

    #[test]
    fn resolves_profile_when_default_uses_provider_model_id_instead_of_profile_key() {
        let mut cfg = AppConfig::default();
        cfg.providers
            .get_mut("openai-responses")
            .expect("openai-responses provider exists")
            .models
            .insert(
                "custom_key".to_string(),
                super::schema::ProviderModelConfig {
                    model: "gpt-5.4".to_string(),
                    enabled: true,
                    request: ModelRequestConfig::default(),
                },
            );
        cfg.default_model = "openai-responses/gpt-5.4".to_string();

        let normalized = cfg.normalize();
        let resolved = normalized
            .resolve_model_profile(None)
            .expect("resolve by model id");
        assert_eq!(resolved.model_id, "gpt-5.4");
    }

    #[test]
    fn load_config_does_not_rewrite_file_on_startup() {
        let _guard = test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let home =
            std::env::temp_dir().join(format!("agentjax-config-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&home).expect("create temp home");
        let path = home.join("config.yaml");
        let raw = [
            "active_provider: \"cm\"",
            "default_model: \"cm/gpt-5.4\"",
            "utility_small_model: \"cm/gpt-5.4\"",
            "request_timeout_seconds: 77",
            "system_prompt: \" custom prompt \"",
            "providers:",
            "  cm:",
            "    kind: \"codex\"",
            "    api_endpoint: \"https://example.com/v1\"",
            "    realtime_endpoint: \"\"",
            "    stream_transport: \"websocket\"",
            "    credential: \"\"",
            "    credential_env: \"CM_API_KEY\"",
            "    request_timeout_seconds: 66",
            "    models:",
            "      profile_a:",
            "        model: \"gpt-5.4\"",
            "        enabled: true",
            "        request:",
            "          reasoning_effort: \"high\"",
            "          extra_body: {}",
            "mcp_runtime:",
            "  stdio:",
            "    inherit_parent_env: false",
            "    env: {}",
            "mcp_servers: {}",
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
        let mut cfg = AppConfig::default();
        let provider = cfg
            .providers
            .get_mut("openai-responses")
            .expect("openai-responses provider exists");
        provider.supports_websockets = false;
        provider.stream_transport = "websocket".to_string();

        let normalized = cfg.normalize();
        let provider = normalized
            .providers
            .get("openai-responses")
            .expect("normalized provider exists");
        assert_eq!(provider.stream_transport, "sse");
    }

    #[test]
    fn provider_resolved_http_headers_merges_env_values() {
        let _guard = test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut provider = ProviderConfig::default();
        provider
            .http_headers
            .insert("X-Feature".to_string(), "static".to_string());
        provider.env_http_headers.insert(
            "Authorization".to_string(),
            "TEST_AGENTJAX_AUTH".to_string(),
        );
        provider.env_http_headers.insert(
            "X-Feature".to_string(),
            "TEST_AGENTJAX_X_FEATURE".to_string(),
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
        cfg.mcp_servers.insert(
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
            .mcp_servers
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
        cfg.mcp_servers.insert(
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
            .mcp_servers
            .get("local-demo")
            .expect("normalized mcp server exists");

        assert_eq!(server.uri, None);
        assert_eq!(server.auth_header, None);
        assert!(server.headers.is_empty());
        assert!(server.allow_stateless);
        assert_eq!(server.channel_buffer_capacity, None);
        assert!(server.reinit_on_expired_session);
    }
}
