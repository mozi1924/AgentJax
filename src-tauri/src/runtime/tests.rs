use super::{
    tool_archiving::archive_unavailable_historical_tool_calls,
    tool_parsing::extract_active_tool_names, AgentRuntime,
};
use crate::commands::chat::ChatRequest;
use crate::providers::types::ProviderStreamEvent;
use crate::tools::ToolCatalog;
use serde_json::json;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_tool_loop_smoke_test_from_local_config() {
    if std::env::var("AGENTJAX_REAL_GATEWAY_TEST").ok().as_deref() != Some("1") {
        eprintln!("Skip real gateway smoke test. Set AGENTJAX_REAL_GATEWAY_TEST=1 to enable.");
        return;
    }

    let config = crate::config::load_config().expect("load local config");
    let resolved_model = config
        .resolve_model_profile(None)
        .expect("resolve default model profile");
    assert!(
        resolved_model.provider.resolved_credential().is_some(),
        "Active/default provider has no resolved credential. Check config.yaml credential or credential_env."
    );

    let conversation_id = format!("test-real-gateway-{}", Uuid::new_v4());
    crate::conversation_store::ensure_conversation(
        &conversation_id,
        config.utility_small_model_key(),
    )
    .expect("ensure conversation workspace");

    let tools_catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
    let req = ChatRequest {
        input: "请先调用 get_system_time 工具获取系统时间，然后用中文给出一句简短结论，并包含“链路测试通过”这六个字。".to_string(),
        conversation_id: Some(conversation_id.clone()),
        model: Some(config.default_model.clone()),
        reasoning_effort: None,
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        request_id: Some(format!("req-real-gateway-{}", Uuid::new_v4())),
    };

    let (_cancel_tx, mut cancel_rx) = watch::channel(false);
    let stream_events: Arc<Mutex<Vec<ProviderStreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let stream_events_for_closure = stream_events.clone();

    let run_result = tokio::time::timeout(
        Duration::from_secs(180),
        AgentRuntime::run_turn(
            &config,
            &req,
            &conversation_id,
            Vec::new(),
            &tools_catalog,
            &mut cancel_rx,
            move |event| {
                stream_events_for_closure
                    .lock()
                    .expect("lock stream events")
                    .push(event.clone());
                Ok(())
            },
        ),
    )
    .await
    .expect("real gateway run_turn timed out")
    .expect("run_turn failed");

    let (response, timeline_events) = run_result;
    assert!(
        !response.output_text.trim().is_empty(),
        "Assistant output should not be empty"
    );

    let has_system_time_tool = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("get_system_time")
    });
    assert!(
        has_system_time_tool,
        "Expected get_system_time tool call in timeline events"
    );

    let has_tool_executed_event = stream_events
        .lock()
        .expect("lock stream events for assert")
        .iter()
        .any(|event| matches!(event, ProviderStreamEvent::ToolCallExecuted { .. }));
    assert!(
        has_tool_executed_event,
        "Expected ToolCallExecuted event in provider stream"
    );

    assert!(
        response.output_text.contains("链路测试通过"),
        "Assistant output should include verification phrase. Actual: {}",
        response.output_text
    );
}

#[test]
fn archives_unavailable_tool_call_pairs_into_developer_note() {
    let active_tools = extract_active_tool_names(&[json!({
        "type": "function",
        "name": "calculator",
        "description": "",
        "parameters": {"type":"object"}
    })]);

    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}),
        json!({"type":"function_call","call_id":"call_old","name":"mcp__github__search_repos","arguments":"{\"q\":\"agent\"}"}),
        json!({"type":"function_call_output","call_id":"call_old","output":"{\"ok\":true,\"result\":[1,2]}"}),
        json!({"type":"function_call","call_id":"call_keep","name":"calculator","arguments":"{\"expression\":\"1+1\"}"}),
        json!({"type":"function_call_output","call_id":"call_keep","output":"{\"ok\":true,\"result\":2}"}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);
    assert!(
        normalized.iter().any(|item| {
            item.get("role").and_then(|v| v.as_str()) == Some("developer")
                && item
                    .get("content")
                    .and_then(|v| v.as_array())
                    .and_then(|arr| arr.first())
                    .and_then(|part| part.get("text"))
                    .and_then(|v| v.as_str())
                    .map(|text| text.contains("ARCHIVED_TOOL_CALL"))
                    .unwrap_or(false)
        }),
        "expected a developer archived-tool note"
    );
    assert!(
        !normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_old")
        }),
        "unavailable historical function_call should be removed from executable context items"
    );
    assert!(
        normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_keep")
        }),
        "available tool call should be preserved"
    );
}
