use super::*;
use crate::agentjax_home::AGENTJAX_HOME_ENV;
use crate::conversation_store_utils::now_unix_ms;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use uuid::Uuid;

struct TestHomeGuard {
    home: std::path::PathBuf,
}

impl Drop for TestHomeGuard {
    fn drop(&mut self) {
        unsafe {
            std::env::remove_var(AGENTJAX_HOME_ENV);
        }
        let _ = std::fs::remove_dir_all(&self.home);
    }
}

fn setup_test_home() -> TestHomeGuard {
    let home = std::env::temp_dir().join(format!(
        "agentjax-conversation-store-test-{}",
        Uuid::new_v4()
    ));
    std::fs::create_dir_all(&home).expect("create test home");
    unsafe {
        std::env::set_var(AGENTJAX_HOME_ENV, &home);
    }
    TestHomeGuard { home }
}

#[test]
fn delete_conversation_removes_session_directory() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-delete-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";

    let path = conversation_dir_path(&conversation_id).expect("path");
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");
    assert!(
        path.exists(),
        "session directory should exist before delete"
    );

    let deleted = delete_conversation(&conversation_id).expect("delete conversation");
    assert!(deleted, "delete should report true when file existed");
    assert!(
        !path.exists(),
        "session directory should be removed after delete"
    );
}

#[test]
fn load_context_merges_history_for_all_providers() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-provider-filter-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "user-openai".to_string(),
            role: "user".to_string(),
            text: "hello openai".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: Some("openai".to_string()),
            model_profile: Some("gpt-5-mini".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-openai".to_string()),
            context_items: build_user_input_items("hello openai"),
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append openai user");

    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "assistant-openai".to_string(),
            role: "assistant".to_string(),
            text: "openai answer".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some("resp-openai".to_string()),
            provider: Some("openai".to_string()),
            model_profile: Some("gpt-5-mini".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-openai".to_string()),
            context_items: build_assistant_output_items("openai answer"),
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append openai assistant");

    let openai_context = load_context_for_request(&conversation_id).expect("openai context");
    assert!(openai_context.input_items.len() >= 2);

    delete_conversation(&conversation_id).expect("cleanup conversation");
}

#[test]
fn load_context_filters_orphan_tool_call_items() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-orphan-tool-items-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    let context_items = vec![
        json!({"type":"function_call","call_id":"call_orphan","name":"tool_a","arguments":{}}),
        json!({"type":"function_call","call_id":"call_ok","name":"tool_b","arguments":{}}),
        json!({"type":"function_call_output","call_id":"call_ok","output":"{\"ok\":true}"}),
    ];

    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "assistant-tool-history".to_string(),
            role: "assistant".to_string(),
            text: "done".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some("resp-tool".to_string()),
            provider: Some("openai".to_string()),
            model_profile: Some("gpt-5-mini".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-tool".to_string()),
            context_items,
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append assistant");

    let context = load_context_for_request(&conversation_id).expect("context");
    assert!(
        !context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some("call_orphan")
        }),
        "orphan function_call should be filtered"
    );
    assert!(
        context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some("call_ok")
        }),
        "paired function_call should be kept"
    );
    assert!(
        context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call_output")
                && item.get("call_id").and_then(Value::as_str) == Some("call_ok")
        }),
        "paired function_call_output should be kept"
    );

    delete_conversation(&conversation_id).expect("cleanup");
}

#[test]
fn load_context_truncates_without_splitting_tool_pairs() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-context-truncate-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    let mut context_items = Vec::new();
    for i in 0..260 {
        context_items.push(json!({
            "role":"user",
            "content":[{"type":"input_text","text": format!("u-{i}")}]
        }));
    }
    context_items.push(
        json!({"type":"function_call","call_id":"call_tail","name":"tool_x","arguments":{}}),
    );
    context_items.push(
        json!({"type":"function_call_output","call_id":"call_tail","output":"{\"ok\":true}"}),
    );

    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "assistant-long-history".to_string(),
            role: "assistant".to_string(),
            text: "done".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some("resp-long".to_string()),
            provider: Some("openai".to_string()),
            model_profile: Some("gpt-5-mini".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-long".to_string()),
            context_items,
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append");

    let context = load_context_for_request(&conversation_id).expect("context");
    assert!(context.input_items.len() <= MAX_CONTEXT_ITEMS_PER_REQUEST);
    assert!(context.input_items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call")
            && item.get("call_id").and_then(Value::as_str) == Some("call_tail")
    }));
    assert!(context.input_items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call_output")
            && item.get("call_id").and_then(Value::as_str) == Some("call_tail")
    }));

    delete_conversation(&conversation_id).expect("cleanup");
}

#[test]
fn append_context_item_keeps_tool_pairs_across_separate_lines() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-context-item-lines-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    append_context_item(
        AppendContextItemInput {
            conversation_id: conversation_id.clone(),
            entry_id: "ctx-call".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: None,
            model_profile: None,
            model_id: None,
            request_id: Some("req-1".to_string()),
            context_item: json!({
                "type":"function_call",
                "call_id":"call_1",
                "name":"mcp__demo__tool",
                "arguments":"{\"x\":1}"
            }),
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append function_call line");

    append_context_item(
        AppendContextItemInput {
            conversation_id: conversation_id.clone(),
            entry_id: "ctx-output".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: None,
            model_profile: None,
            model_id: None,
            request_id: Some("req-1".to_string()),
            context_item: json!({
                "type":"function_call_output",
                "call_id":"call_1",
                "output":"{\"ok\":true}"
            }),
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append function_call_output line");

    let context = load_context_for_request(&conversation_id).expect("load context");
    let has_call = context.input_items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call")
            && item.get("call_id").and_then(Value::as_str) == Some("call_1")
    });
    let has_output = context.input_items.iter().any(|item| {
        item.get("type").and_then(Value::as_str) == Some("function_call_output")
            && item.get("call_id").and_then(Value::as_str) == Some("call_1")
    });
    assert!(
        has_call && has_output,
        "expected tool pair restored from separate lines"
    );
}

#[test]
fn build_recovery_note_for_unfinished_turn() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-recovery-note-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "msg-user-1".to_string(),
            role: "user".to_string(),
            text: "请继续".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: None,
            model_profile: None,
            model_id: None,
            request_id: Some("req-recover".to_string()),
            context_items: build_user_input_items("请继续"),
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append user");

    append_context_item(
        AppendContextItemInput {
            conversation_id: conversation_id.clone(),
            entry_id: "ctx-call-recover".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: None,
            model_profile: None,
            model_id: None,
            request_id: Some("req-recover".to_string()),
            context_item: json!({
                "type":"function_call",
                "call_id":"call_recover_1",
                "name":"mcp__demo__tool",
                "arguments":"{\"x\":1}"
            }),
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append call");

    let note = build_recovery_developer_note(&conversation_id)
        .expect("build recovery note")
        .expect("expected recovery note");
    let note_text = note
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(note_text.contains("RECOVERY_CONTEXT"));
    assert!(note_text.contains("req-recover"));
    assert!(note_text.contains("call_recover_1"));
}

#[test]
fn load_conversation_includes_tool_context_item_lines() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-load-context-lines-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    append_context_item(
        AppendContextItemInput {
            conversation_id: conversation_id.clone(),
            entry_id: "ctx-tool-line".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some("resp-1".to_string()),
            provider: None,
            model_profile: None,
            model_id: None,
            request_id: Some("req-ctx-line".to_string()),
            context_item: json!({
                "type":"function_call_output",
                "call_id":"call_ctx_1",
                "output":"{\"ok\":true}"
            }),
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append context item");

    let detail = load_conversation(&conversation_id)
        .expect("load conversation")
        .expect("detail");
    assert!(detail.messages.iter().any(|msg| {
        msg.id == "ctx-tool-line"
            && msg.role == "assistant"
            && msg.context_items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call_output")
            })
    }));
}

#[test]
fn fault_injection_interrupted_tool_turn_can_recover_and_clear() {
    let _guard = crate::config::test_env_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let _home = setup_test_home();
    let conversation_id = format!("test-fault-injection-{}", Uuid::new_v4());
    let utility_model = "gpt-5-mini";
    ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

    // Step 1: user message persisted, tool_call persisted, then crash before tool_output/assistant.
    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "msg-user-fault".to_string(),
            role: "user".to_string(),
            text: "请调用工具然后继续".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: Some("codex".to_string()),
            model_profile: Some("default".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-fault".to_string()),
            context_items: build_user_input_items("请调用工具然后继续"),
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append user");

    append_context_item(
        AppendContextItemInput {
            conversation_id: conversation_id.clone(),
            entry_id: "ctx-call-fault".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: Some("codex".to_string()),
            model_profile: Some("default".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-fault".to_string()),
            context_item: json!({
                "type":"function_call",
                "call_id":"call_fault_1",
                "name":"mcp__demo__search",
                "arguments":"{\"q\":\"agentjax\"}"
            }),
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append function_call");

    // Step 2: restart path should emit recovery note with unresolved tool call.
    let note_before_resume = build_recovery_developer_note(&conversation_id)
        .expect("build recovery note before resume")
        .expect("expected recovery note before resume");
    let note_text = note_before_resume
        .get("content")
        .and_then(Value::as_array)
        .and_then(|arr| arr.first())
        .and_then(|part| part.get("text"))
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(note_text.contains("RECOVERY_CONTEXT"));
    assert!(note_text.contains("call_fault_1"));
    assert!(note_text.contains("assistant_message_missing"));

    // Step 3: resume execution completes tool_output and assistant.
    append_context_item(
        AppendContextItemInput {
            conversation_id: conversation_id.clone(),
            entry_id: "ctx-output-fault".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some("resp-fault-1".to_string()),
            provider: Some("codex".to_string()),
            model_profile: Some("default".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-fault".to_string()),
            context_item: json!({
                "type":"function_call_output",
                "call_id":"call_fault_1",
                "output":"{\"ok\":true,\"result\":{\"hits\":3}}"
            }),
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append function_call_output");

    append_message(
        AppendMessageInput {
            conversation_id: conversation_id.clone(),
            entry_id: "msg-assistant-fault".to_string(),
            role: "assistant".to_string(),
            text: "工具执行完成，继续回答。".to_string(),
            created_at_unix_ms: now_unix_ms(),
            response_id: Some("resp-fault-1".to_string()),
            provider: Some("codex".to_string()),
            model_profile: Some("default".to_string()),
            model_id: Some("gpt-5-mini".to_string()),
            request_id: Some("req-fault".to_string()),
            context_items: build_assistant_output_items("工具执行完成，继续回答。"),
            timeline_events: None,
            metadata: BTreeMap::new(),
        },
        utility_model,
    )
    .expect("append assistant");

    let note_after_resume =
        build_recovery_developer_note(&conversation_id).expect("build recovery after resume");
    assert!(
        note_after_resume.is_none(),
        "recovery note should be cleared after tool output and assistant message are present"
    );
}
