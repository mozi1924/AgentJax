use super::{
    AgentRuntime, tool_archiving::archive_unavailable_historical_tool_calls,
    tool_parsing::extract_active_tool_names,
};
use crate::commands::chat::ChatRequest;
use crate::config::{AppConfig, PromptBlock, PromptBlockRole, PromptBlockSource};
use crate::message_phase::AssistantPhase;
use crate::provider_api::types::ProviderStreamEvent;
use crate::tools::ToolCatalog;
use serde_json::json;
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

static RUSTLS_CRYPTO_PROVIDER: Once = Once::new();

fn ensure_rustls_crypto_provider() {
    RUSTLS_CRYPTO_PROVIDER.call_once(|| {
        rustls::crypto::ring::default_provider()
            .install_default()
            .expect("Failed to install rustls ring crypto provider for tests");
    });
}

fn real_gateway_test_enabled() -> bool {
    if std::env::var("AGENTJAX_REAL_GATEWAY_TEST").ok().as_deref() != Some("1") {
        eprintln!("Skip real gateway smoke test. Set AGENTJAX_REAL_GATEWAY_TEST=1 to enable.");
        return false;
    }
    true
}

fn normalize_for_overlap_check(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

async fn run_real_gateway_turn(
    input: &str,
) -> (
    crate::provider_api::types::ResponseStreamResult,
    Vec<serde_json::Value>,
    Vec<ProviderStreamEvent>,
) {
    ensure_rustls_crypto_provider();

    let config = crate::config::load_config().expect("load local config");
    run_real_gateway_turn_with_config(config, input).await
}

async fn run_real_gateway_turn_with_config(
    config: AppConfig,
    input: &str,
) -> (
    crate::provider_api::types::ResponseStreamResult,
    Vec<serde_json::Value>,
    Vec<ProviderStreamEvent>,
) {
    ensure_rustls_crypto_provider();
    let resolved_model = config
        .resolve_model_profile(None)
        .expect("resolve default model profile");
    assert!(
        resolved_model.provider.resolved_credential().is_some(),
        "Active/default provider has no resolved credential. Check config.yaml credential or credential_env."
    );

    let conversation_id = format!("test-real-gateway-{}", Uuid::new_v4());
    crate::conversation_store::ensure_conversation(&conversation_id)
        .expect("ensure conversation workspace");

    let tools_catalog = ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config);
    let req = ChatRequest {
        input: input.to_string(),
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
            crate::conversation_store_utils::now_unix_ms(),
            Vec::new(),
            None,
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
    let stream_events = stream_events
        .lock()
        .expect("lock stream events after run")
        .clone();

    (response, timeline_events, stream_events)
}

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_tool_loop_smoke_test_from_local_config() {
    if !real_gateway_test_enabled() {
        return;
    }

    let (response, timeline_events, stream_events) = run_real_gateway_turn(
        "请先调用 get_system_time 工具获取系统时间，然后用中文给出一句简短结论，并包含“链路测试通过”这六个字。",
    )
    .await;
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

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_multihop_commentary_and_multi_tool_smoke_test_from_local_config() {
    if !real_gateway_test_enabled() {
        return;
    }

    let prompt = concat!(
        "请严格按顺序完成下面任务，并全程使用中文：",
        "第一步，先输出一句简短旁白，明确说明你现在要获取系统时间。",
        "第二步，调用 get_system_time。",
        "第三步，拿到结果后，再输出一句新的简短旁白，明确说明你现在要计算 12*(3+4)。",
        "第四步，调用 calculator，且 expression 必须精确等于 \"12*(3+4)\"。",
        "第五步，最后输出一句最终回答，必须同时包含“多段旁白验证通过”和“84”，并且不要调用任何其他工具。"
    );

    let (response, timeline_events, stream_events) = run_real_gateway_turn(prompt).await;
    assert!(
        !response.output_text.trim().is_empty(),
        "Assistant output should not be empty"
    );

    let tool_names: Vec<&str> = timeline_events
        .iter()
        .filter(|event| event.get("type").and_then(|v| v.as_str()) == Some("toolCall"))
        .filter_map(|event| event.get("name").and_then(|v| v.as_str()))
        .collect();
    assert_eq!(
        tool_names,
        vec!["get_system_time", "calculator"],
        "Expected exactly two tool calls in order. Actual: {:?}",
        tool_names
    );

    let calculator_arguments = timeline_events
        .iter()
        .find(|event| event.get("name").and_then(|v| v.as_str()) == Some("calculator"))
        .and_then(|event| event.get("arguments"))
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        calculator_arguments
            .get("expression")
            .and_then(|v| v.as_str()),
        Some("12*(3+4)"),
        "Calculator should be called with the expected expression. Actual: {}",
        calculator_arguments
    );

    let commentary_messages: Vec<String> = stream_events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::HopAssistantText { text, phase, .. }
                if *phase == Some(AssistantPhase::Commentary) =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    eprintln!("tool_names={:?}", tool_names);
    eprintln!("commentary_messages={:?}", commentary_messages);

    let final_messages: Vec<String> = stream_events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::HopAssistantText { text, phase, .. }
                if *phase != Some(AssistantPhase::Commentary) =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    eprintln!("final_messages={:?}", final_messages);
    eprintln!("final_output_text={}", response.output_text);
    eprintln!("final_output_text_debug={:?}", response.output_text);
    assert!(
        commentary_messages.len() >= 2,
        "Expected at least two commentary hop messages. Actual: {:?}",
        commentary_messages
    );
    assert!(
        commentary_messages
            .iter()
            .any(|text| text.contains("系统时间")),
        "Expected one commentary message to mention system time. Actual: {:?}",
        commentary_messages
    );
    assert!(
        commentary_messages
            .iter()
            .any(|text| text.contains("12*(3+4)") || text.contains("84")),
        "Expected one commentary message to mention the calculator step. Actual: {:?}",
        commentary_messages
    );
    assert!(
        !final_messages.is_empty(),
        "Expected at least one final-answer hop message"
    );
    let last_final_message = final_messages
        .last()
        .expect("expected at least one final message");
    assert_eq!(
        response.output_text.trim(),
        last_final_message.trim(),
        "Response output_text should match the final streamed assistant message"
    );
    let commentary_norms: Vec<String> = commentary_messages
        .iter()
        .map(|text| normalize_for_overlap_check(text))
        .collect();
    for final_message in &final_messages {
        for final_line in final_message.lines() {
            let normalized = normalize_for_overlap_check(final_line);
            assert!(
                commentary_norms
                    .iter()
                    .all(|commentary| commentary != &normalized),
                "Final message should not repeat a commentary line verbatim. final={:?} commentary={:?}",
                final_messages,
                commentary_messages
            );
        }
    }

    let executed_tool_names: Vec<&str> = stream_events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::ToolCallExecuted { name, .. } => Some(name.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        executed_tool_names,
        vec!["get_system_time", "calculator"],
        "Expected both tools to execute in order. Actual: {:?}",
        executed_tool_names
    );

    assert!(
        response.output_text.contains("多段旁白验证通过") && response.output_text.contains("84"),
        "Final answer should contain the expected verification text. Actual: {}",
        response.output_text
    );
}

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_prompt_composer_blocks_smoke_test_from_local_config() {
    if !real_gateway_test_enabled() {
        return;
    }

    let mut config = crate::config::load_config().expect("load local config");
    config.prompt_composer.blocks.push(PromptBlock {
        id: "test-system-block".to_string(),
        title: "Test system block".to_string(),
        role: PromptBlockRole::System,
        content: "Keep the final answer to exactly one Chinese sentence.".to_string(),
        enabled: true,
        source: PromptBlockSource::User,
        source_id: None,
        locked: false,
    });
    config.prompt_composer.blocks.push(PromptBlock {
        id: "test-developer-block".to_string(),
        title: "Test developer block".to_string(),
        role: PromptBlockRole::Developer,
        content:
            "Before any tool call, output one short Chinese commentary line containing the exact phrase 来自developer块."
                .to_string(),
        enabled: true,
        source: PromptBlockSource::User,
        source_id: None,
        locked: false,
    });
    config = config.normalize();

    let (response, timeline_events, stream_events) = run_real_gateway_turn_with_config(
        config,
        "请调用 get_system_time，然后用中文给出最终结论，并包含“组合提示词验证通过”。",
    )
    .await;

    let has_system_time_tool = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("get_system_time")
    });
    assert!(
        has_system_time_tool,
        "Expected get_system_time tool call in timeline events"
    );

    let commentary_messages: Vec<String> = stream_events
        .iter()
        .filter_map(|event| match event {
            ProviderStreamEvent::HopAssistantText { text, phase, .. }
                if *phase == Some(AssistantPhase::Commentary) =>
            {
                Some(text.clone())
            }
            _ => None,
        })
        .collect();
    assert!(
        commentary_messages
            .iter()
            .any(|text| text.contains("来自developer块")),
        "Expected at least one commentary message influenced by developer prompt blocks. Actual: {:?}",
        commentary_messages
    );
    assert!(
        response.output_text.contains("组合提示词验证通过"),
        "Expected final output to contain verification phrase. Actual: {}",
        response.output_text
    );
    assert_eq!(
        response.output_text.lines().count(),
        1,
        "System prompt block should encourage a single-sentence final answer. Actual: {:?}",
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
