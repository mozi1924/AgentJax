use super::*;
use crate::agentjax_home::AGENTJAX_HOME_ENV;
use serde_json::Value;
use std::fs;
use std::path::Path;

fn write_test_config(home: &Path) -> std::path::PathBuf {
    let path = home.join("config.yaml");
    let raw = [
        "active_provider: \"openai\"",
        "default_model: \"openai/gpt-5-mini\"",
        "utility_small_model: \"openai/gpt-5-mini\"",
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
        "  openai:",
        "    kind: \"openai\"",
        "    apiEndpoint: \"https://api.openai.com/v1\"",
        "    realtimeEndpoint: \"\"",
        "    streamTransport: \"websocket\"",
        "    credential: \"SECRET\"",
        "    credentialEnv: \"OPENAI_API_KEY\"",
        "    requestTimeoutSeconds: 120",
        "    models:",
        "      gpt-5-mini:",
        "        enabled: true",
        "        request:",
        "          reasoning:",
        "          extra_body: {}",
        "mcp:",
        "  stdio:",
        "    inherit_parent_env: false",
        "    env: {}",
        "  startup_timeout_ms: 15000",
        "  tool_timeout_ms: 60000",
        "  servers: {}",
        "",
    ]
    .join("\n");
    fs::write(&path, raw).expect("write config");
    path
}

#[test]
fn snapshot_redacts_secret_values() {
    let _guard = crate::config::test_env_lock().blocking_lock();
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let _path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot(None).expect("snapshot");
    assert_eq!(
        snapshot.values["providers"]["openai"]["credential"],
        Value::Null
    );
    assert_eq!(
        snapshot
            .secret_statuses
            .get("providers.openai.credential")
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
    let _guard = crate::config::test_env_lock().blocking_lock();
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot(None).expect("snapshot");
    let updated = apply_settings_patch(SettingsPatch {
        path: "request_timeout_seconds".to_string(),
        value: Some(Value::from(33)),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
        agent_id: None,
    })
    .expect("apply patch");

    assert_eq!(updated.values["request_timeout_seconds"], Value::from(33));
    // agent-specific paths are written to agent.yaml
    let agent_path = path
        .parent()
        .unwrap()
        .join("agents")
        .join("main")
        .join("agent.yaml");
    if agent_path.exists() {
        let raw = fs::read_to_string(&agent_path).expect("read agent config");
        assert!(raw.contains("request_timeout_seconds: 33"));
    }

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn apply_patch_rejects_invalid_collection_keys() {
    let _guard = crate::config::test_env_lock().blocking_lock();
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let _path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot(None).expect("snapshot");
    let error = apply_settings_patch(SettingsPatch {
        path: "mcp.servers.bad$key".to_string(),
        value: Some(serde_json::json!({ "transport": "stdio", "enabled": true })),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
        agent_id: None,
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
    let _guard = crate::config::test_env_lock().blocking_lock();
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot(None).expect("snapshot");
    let updated = apply_settings_patch(SettingsPatch {
        path: "tool_manager.native_tools.calculator.enabled".to_string(),
        value: Some(Value::Bool(false)),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
        agent_id: None,
    })
    .expect("apply tool manager patch");

    assert_eq!(
        updated.values["tool_manager"]["native_tools"]["calculator"]["enabled"],
        Value::Bool(false)
    );
    // agent-specific paths are written to agent.yaml
    let agent_path = path
        .parent()
        .unwrap()
        .join("agents")
        .join("main")
        .join("agent.yaml");
    if agent_path.exists() {
        let raw = fs::read_to_string(&agent_path).expect("read agent config");
        assert!(raw.contains("tool_manager:"));
        assert!(raw.contains("calculator:"));
    }

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}

#[test]
fn apply_patch_supports_escaped_model_profile_keys_with_dots() {
    let _guard = crate::config::test_env_lock().blocking_lock();
    let home =
        std::env::temp_dir().join(format!("agentjax-settings-test-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&home).expect("create home");
    let path = write_test_config(&home);

    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }

    let snapshot = get_settings_snapshot(None).expect("snapshot");
    let updated = apply_settings_patch(SettingsPatch {
        path: "providers.openai.models.GPT-5\\.4-mini.enabled".to_string(),
        value: Some(Value::Bool(false)),
        expected_revision: snapshot.revision,
        operation: SettingsPatchOperation::Set,
        agent_id: None,
    })
    .expect("apply patch with escaped model profile key");

    assert_eq!(
        updated.values["providers"]["openai"]["models"]["GPT-5.4-mini"]["enabled"],
        Value::Bool(false)
    );
    let raw = fs::read_to_string(&path).expect("read config");
    assert!(raw.contains("GPT-5.4-mini:"));

    unsafe {
        std::env::remove_var(AGENTJAX_HOME_ENV);
    }
    let _ = fs::remove_dir_all(home);
}
