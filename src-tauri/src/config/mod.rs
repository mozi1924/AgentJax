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
}
