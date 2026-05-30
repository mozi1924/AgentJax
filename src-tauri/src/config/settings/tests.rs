use super::*;
use crate::agentjax_home::AGENTJAX_HOME_ENV;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn write_test_config(home: &Path) -> std::path::PathBuf {
    let path = home.join("config.yaml");
    let raw = [
        "active_provider: \"openai-responses\"",
        "default_model: \"openai-responses/gpt-5-mini\"",
        "utility_small_model: \"openai-responses/gpt-5-mini\"",
        "request_timeout_seconds: 120",
        "prompt_composer:",
        "  blocks:",
        "    - id: \"user-system\"",
        "      title: \"User system\"",
        "      role: \"system\"",
        "      content: \"Assistant\"",
        "      enabled: true",
        "      source: \"user\"",
        "      source_id: null",
        "      locked: false",
        "providers:",
        "  openai-responses:",
        "    kind: \"openai-responses\"",
        "    api_endpoint: \"https://api.openai.com/v1\"",
        "    realtime_endpoint: \"\"",
        "    stream_transport: \"websocket\"",
        "    credential: \"SECRET\"",
        "    credential_env: \"OPENAI_API_KEY\"",
        "    request_timeout_seconds: 120",
        "    models:",
        "      gpt-5-mini:",
        "        model: \"gpt-5-mini\"",
        "        enabled: true",
        "        request:",
        "          reasoning_effort: null",
        "          extra_body: {}",
        "mcp_runtime:",
        "  stdio:",
        "    inherit_parent_env: false",
        "    env: {}",
        "  startup_timeout_ms: 15000",
        "  tool_timeout_ms: 60000",
        "mcp_servers: {}",
        "",
    ]
    .join("\n");
    fs::write(&path, raw).expect("write config");
    path
}

#[test]
fn snapshot_redacts_secret_values() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let _path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot().expect("snapshot");
    assert_eq!(
        snapshot.values["providers"]["openai-responses"]["credential"],
        Value::Null
    );
    assert_eq!(
        snapshot
            .secret_statuses
            .get("providers.openai-responses.credential")
            .expect("secret status")
            .source,
        "inline"
    );

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn apply_patch_updates_scalar_values() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot().expect("snapshot");
    let updated = apply_settings_patch(SettingsPatch {
        path: "request_timeout_seconds".to_string(),
        value: Some(Value::from(33)),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
    })
    .expect("apply patch");

    assert_eq!(updated.values["request_timeout_seconds"], Value::from(33));
    let raw = fs::read_to_string(&path).expect("read config");
    assert!(raw.contains("request_timeout_seconds: 33"));

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn apply_patch_rejects_invalid_collection_keys() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let _path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot().expect("snapshot");
    let error = apply_settings_patch(SettingsPatch {
        path: "mcp_servers.bad$key".to_string(),
        value: Some(serde_json::json!({ "transport": "stdio", "enabled": true })),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
    })
    .expect_err("invalid key should fail");
    assert!(error.contains("unsupported characters"));

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn apply_patch_updates_tool_manager_policy() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot().expect("snapshot");
    let updated = apply_settings_patch(SettingsPatch {
        path: "tool_manager.native_tools.calculator.enabled".to_string(),
        value: Some(Value::Bool(false)),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
    })
    .expect("apply tool manager patch");

    assert_eq!(
        updated.values["tool_manager"]["native_tools"]["calculator"]["enabled"],
        Value::Bool(false)
    );
    let raw = fs::read_to_string(&path).expect("read config");
    assert!(raw.contains("tool_manager:"));
    assert!(raw.contains("calculator:"));

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn apply_patch_supports_escaped_model_profile_keys_with_dots() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot().expect("snapshot");
    let updated = apply_settings_patch(SettingsPatch {
        path: "providers.openai-responses.models.GPT-5\\.4.model".to_string(),
        value: Some(Value::from("gpt-5.4")),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
    })
    .expect("apply patch with escaped model profile key");

    assert_eq!(
        updated.values["providers"]["openai-responses"]["models"]["GPT-5.4"]["model"],
        Value::from("gpt-5.4")
    );
    let raw = fs::read_to_string(&path).expect("read config");
    assert!(raw.contains("GPT-5.4:"));

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}
