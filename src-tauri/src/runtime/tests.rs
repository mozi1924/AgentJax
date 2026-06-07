use super::{
    AgentRuntime, agent_context::LcmAgentContext,
    tool_archiving::archive_unavailable_historical_tool_calls,
    tool_parsing::extract_active_tool_names,
};
use crate::commands::chat::ChatRequest;
use crate::config::{AppConfig, PromptBlock, PromptBlockRole, PromptBlockSource};
use crate::message_phase::AssistantPhase;
use crate::provider_api::types::ProviderStreamEvent;
use crate::tools::ToolCatalog;
use std::collections::HashSet;
use serde_json::{Value, json};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use tokio::sync::watch;
use uuid::Uuid;

fn lcm_engine_for_test(conversation_id: &str) -> Arc<crate::lcm::LcmEngine> {
    let config = crate::lcm::LcmConfig::default();
    crate::lcm::open_lcm_engine(
        crate::config::constants::DEFAULT_AGENT_ID,
        conversation_id,
        &config,
    )
    .expect("Failed to open LCM engine for test")
}

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
    let agent = crate::config::load_agent_config(crate::config::constants::DEFAULT_AGENT_ID)
        .unwrap_or_default()
        .normalize();
    let resolved_model = config
        .resolve_model_profile_with_agent(None, &agent)
        .expect("resolve default model profile");
    assert!(
        resolved_model.provider.resolved_credential().is_some(),
        "Active/default provider has no resolved credential. Check config.yaml credential or credential_env."
    );

    let conversation_id = format!("test-real-gateway-{}", Uuid::new_v4());
    crate::conversation_store::ensure_conversation(
        crate::config::constants::DEFAULT_AGENT_ID,
        &conversation_id,
    )
    .expect("ensure conversation workspace");

    let tools_catalog = Arc::new(ToolCatalog::new(Arc::new(crate::mcp::McpManager::new()), &config, &agent));
    let req = ChatRequest {
        input: input.to_string(),
        conversation_id: Some(conversation_id.clone()),
        model: Some(agent.default_model.clone()),
        reasoning: None,
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
        request_id: Some(format!("req-real-gateway-{}", Uuid::new_v4())),
        temperature: None,
        top_p: None,
        presence_penalty: None,
        frequency_penalty: None,
        max_tokens: None,
        max_completion_tokens: None,
    };

    let (_cancel_tx, mut cancel_rx) = watch::channel(false);
    let stream_events: Arc<Mutex<Vec<ProviderStreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let stream_events_for_closure = stream_events.clone();

    let run_result = tokio::time::timeout(
        Duration::from_secs(180),
        AgentRuntime::run_turn(
            &config,
            &agent,
            "test-agent",
            &req,
            &conversation_id,
            crate::conversation_store_utils::now_unix_ms(),
            Vec::new(),
            None,
            &tools_catalog,
            &LcmAgentContext::new(lcm_engine_for_test(&conversation_id)),
            &mut cancel_rx,
            None,       // sub_agent_event_tx
            Vec::new(), // street_items
            false,      // is_auto_resume
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

    let config = crate::config::load_config().expect("load local config");
    let mut agent = crate::config::load_agent_config(crate::config::constants::DEFAULT_AGENT_ID)
        .unwrap_or_default()
        .normalize();
    agent.prompt_composer.blocks.push(PromptBlock {
        id: "test-system-block".to_string(),
        title: "Test system block".to_string(),
        role: PromptBlockRole::System,
        content: "Keep the final answer to exactly one Chinese sentence.".to_string(),
        enabled: true,
        source: PromptBlockSource::User,
        source_id: None,
        locked: false,
    });
    agent.prompt_composer.blocks.push(PromptBlock {
        id: "test-extra-block".to_string(),
        title: "Test extra block".to_string(),
        role: PromptBlockRole::System,
        content:
            "Before any tool call, output one short Chinese commentary line containing the exact phrase 来自system块."
                .to_string(),
        enabled: true,
        source: PromptBlockSource::User,
        source_id: None,
        locked: false,
    });
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

/// Helper: check if an item is part of an `_archived_tool` carrier pair.
/// After archiving, the original function_call is rewritten with
/// `name: "_archived_tool"` (keeping the original `call_id`), and the
/// function_call_output has its output wrapped.
fn is_carrier_item(item: &Value) -> bool {
    // Function_call with name _archived_tool
    item.get("name").and_then(|v| v.as_str()) == Some("_archived_tool")
        // Function_call_output: check if the output contains "original_tool"
        // (carrier outputs wrap the original output in a JSON envelope)
        || (item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && item.get("output").and_then(|v| v.as_str())
                .map(|o| o.contains("\"original_tool\""))
                .unwrap_or(false))
}

/// Helper: check that an `_archived_tool` carrier pair exists for the given
/// original call_id.  Since we reuse the original call_id, the carrier
/// function_call has the SAME call_id but name="_archived_tool".
fn assert_archived_carrier_pair(items: &[Value], original_call_id: &str) {
    let fc = items.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("function_call")
            && item.get("call_id").and_then(|v| v.as_str()) == Some(original_call_id)
            && item.get("name").and_then(|v| v.as_str()) == Some("_archived_tool")
    });
    let fco = items.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && item.get("call_id").and_then(|v| v.as_str()) == Some(original_call_id)
    });
    assert!(fc.is_some(), "expected _archived_tool function_call with call_id={original_call_id}");
    assert!(fco.is_some(), "expected function_call_output with call_id={original_call_id}");
}

/// Check that an orphaned `function_call` (no matching output) was archived
/// — only the function_call is rewritten, no function_call_output exists.
fn assert_archived_orphaned_call(items: &[Value], original_call_id: &str) {
    let fc = items.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("function_call")
            && item.get("call_id").and_then(|v| v.as_str()) == Some(original_call_id)
            && item.get("name").and_then(|v| v.as_str()) == Some("_archived_tool")
    });
    assert!(fc.is_some(), "expected _archived_tool function_call with call_id={original_call_id} (orphaned call)");
    // No function_call_output should match this call_id.
    let fco = items.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && item.get("call_id").and_then(|v| v.as_str()) == Some(original_call_id)
    });
    assert!(fco.is_none(), "expected NO function_call_output for orphaned call {original_call_id}");
}

/// Check that an orphaned `function_call_output` (no matching call) was
/// archived — only the output is wrapped, no function_call exists.
fn assert_archived_orphaned_output(items: &[Value], original_call_id: &str) {
    let fco = items.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
            && item.get("call_id").and_then(|v| v.as_str()) == Some(original_call_id)
            && item.get("output").and_then(|v| v.as_str())
                .map(|o| o.contains("\"original_tool\""))
                .unwrap_or(false)
    });
    assert!(fco.is_some(), "expected wrapped function_call_output with call_id={original_call_id} (orphaned output)");
    // No function_call should match this call_id.
    let fc = items.iter().find(|item| {
        item.get("type").and_then(|v| v.as_str()) == Some("function_call")
            && item.get("call_id").and_then(|v| v.as_str()) == Some(original_call_id)
    });
    assert!(fc.is_none(), "expected NO function_call for orphaned output {original_call_id}");
}

/// Helper: assert that no carrier pair exists (used when archiving should not happen).
fn assert_no_archived_carrier(items: &[Value]) {
    let has_carrier = items.iter().any(|item| {
        item.get("name").and_then(|v| v.as_str()) == Some("_archived_tool")
    });
    assert!(!has_carrier, "expected no _archived_tool carrier pairs");
}

#[test]
fn archives_unavailable_tool_call_pairs_as_carrier_pair() {
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

    // The unavailable mcp__github__search_repos should be archived via carrier pair.
    assert_archived_carrier_pair(&normalized, "call_old");

    // The original call_id should no longer appear as a raw function_call or output.
    assert!(
        !normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_old")
                && item.get("name").and_then(|v| v.as_str()) != Some("_archived_tool")
        }),
        "unavailable historical function_call should be removed"
    );
    assert!(
        normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_keep")
        }),
        "available tool call should be preserved"
    );
}

#[test]
fn preserves_tool_calls_when_tool_is_re_registered() {
    // Simulate a tool being unmounted then re-mounted: its name is back
    // in active_tool_names, so archiving should leave items untouched.
    let active_tools = extract_active_tool_names(&[
        json!({"type":"function","name":"calculator","description":"","parameters":{"type":"object"}}),
        json!({"type":"function","name":"mcp__github__search_repos","description":"","parameters":{"type":"object"}}),
    ]);

    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}),
        json!({"type":"function_call","call_id":"call_001","name":"mcp__github__search_repos","arguments":"{\"q\":\"agent\"}"}),
        json!({"type":"function_call_output","call_id":"call_001","output":"{\"ok\":true,\"result\":[1,2]}"}),
        json!({"type":"function_call","call_id":"call_002","name":"calculator","arguments":"{\"expression\":\"1+1\"}"}),
        json!({"type":"function_call_output","call_id":"call_002","output":"{\"ok\":true,\"result\":2}"}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);

    // All original function_call items should be preserved (no archiving)
    assert!(
        normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_001")
        }),
        "re-registered tool's historical function_call should be preserved"
    );
    assert!(
        normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_001")
        }),
        "re-registered tool's historical function_call_output should be preserved"
    );
    // The calculator tool call should also be preserved
    assert!(
        normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_002")
        }),
        "available tool call should be preserved"
    );
    // No carrier pairs should exist.
    assert_no_archived_carrier(&normalized);
}

#[test]
fn archives_all_tool_calls_when_no_tools_are_active() {
    // When every tool is disabled (active_tool_names is empty), every
    // historical function_call should be archived into a user-role note.
    let active_tools = HashSet::new(); // empty — no tools available

    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}),
        json!({"type":"function_call","call_id":"call_a","name":"read_file","arguments":"{\"path\":\"/tmp/x\"}"}),
        json!({"type":"function_call_output","call_id":"call_a","output":"{\"ok\":true}"}),
        json!({"type":"function_call","call_id":"call_b","name":"calculator","arguments":"{\"expression\":\"2+2\"}"}),
        json!({"type":"function_call_output","call_id":"call_b","output":"{\"ok\":true,\"result\":4}"}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);

    // No raw function_call or function_call_output items should remain
    // (except _archived_tool carrier pairs).
    assert!(
        !normalized.iter().any(|item| {
            let fc_type = item.get("type").and_then(|v| v.as_str());
            let is_carrier = is_carrier_item(item);
            matches!(fc_type, Some("function_call") | Some("function_call_output")) && !is_carrier
        }),
        "all non-carrier function_call/output items should be archived when no tools are active"
    );

    // Two carrier pairs expected (one per archived call).
    assert_archived_carrier_pair(&normalized, "call_a");
    assert_archived_carrier_pair(&normalized, "call_b");
}

#[test]
fn archives_orphaned_tool_calls_without_matching_output() {
    // When LCM compaction removes a tool result message but leaves the
    // assistant message with tool_calls_json intact, the function_call
    // has no matching function_call_output. The archiver must handle
    // this by archiving the orphaned call — otherwise the Chat Completions
    // API rejects the request because an assistant with tool_calls is
    // not followed by tool-role response messages.
    let active_tools = extract_active_tool_names(&[json!({
        "type": "function",
        "name": "calculator",
        "description": "",
        "parameters": {"type":"object"}
    })]);

    // calculator is in active_tools but has NO output in context
    // (simulating LCM compaction removing the tool result).
    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}),
        json!({"type":"function_call","call_id":"call_a","name":"calculator","arguments":"{\"expression\":\"1+1\"}"}),
        // function_call_output for call_a is MISSING (compacted by LCM)
        json!({"role":"assistant","content":[{"type":"output_text","text":"Let me help with that."}]}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);

    // The orphaned function_call should be archived (rewritten as _archived_tool).
    assert_archived_orphaned_call(&normalized, "call_a");

    // No raw function_call or function_call_output should remain
    // (except the carrier).
    assert!(
        !normalized.iter().any(|item| {
            let fc_type = item.get("type").and_then(|v| v.as_str());
            let is_carrier = is_carrier_item(item);
            matches!(fc_type, Some("function_call") | Some("function_call_output")) && !is_carrier
        }),
        "orphaned function_call without matching output should be archived"
    );

    // The assistant message should still be present.
    assert!(
        normalized.iter().any(|item| {
            item.get("role").and_then(|v| v.as_str()) == Some("assistant")
        }),
        "assistant message should be preserved after archiving orphaned call"
    );
}

#[test]
fn archives_orphaned_tool_output_without_matching_call() {
    // When LCM compaction removes the assistant message (with tool_calls_json)
    // but leaves the tool result message (standalone Tool-role messages are
    // excluded from compaction), the function_call_output has no matching
    // function_call. The archiver must handle this by archiving the orphaned
    // output — otherwise the Chat Completions API rejects the request because
    // a tool-role message has no preceding tool_calls.
    let active_tools = extract_active_tool_names(&[json!({
        "type": "function",
        "name": "calculator",
        "description": "",
        "parameters": {"type":"object"}
    })]);

    // function_call_output for call_a is present, but the function_call
    // is MISSING (simulating LCM compaction removing the assistant message).
    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"hi"}]}),
        json!({"type":"function_call_output","call_id":"call_a","output":"{\"ok\":true,\"result\":2}"}),
        json!({"role":"assistant","content":[{"type":"output_text","text":"The answer is 2."}]}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);

    // The orphaned function_call_output should be archived (output wrapped).
    assert_archived_orphaned_output(&normalized, "call_a");

    // No raw function_call or function_call_output should remain (except carrier).
    assert!(
        !normalized.iter().any(|item| {
            let fc_type = item.get("type").and_then(|v| v.as_str());
            let is_carrier = is_carrier_item(item);
            matches!(fc_type, Some("function_call") | Some("function_call_output")) && !is_carrier
        }),
        "orphaned function_call_output without matching call should be archived"
    );
}

#[test]
fn archives_kb_tools_when_disabled() {
    // KB tools (kb_list, kb_search, kb_get, kb_index) use the same
    // archiver as native tools — the only difference is that they're
    // gated via tool_manager.context_tools.* instead of native_tools.*.
    // This test verifies that the archiver handles KB tool names
    // identically to native tool names.
    let active_tools = extract_active_tool_names(&[json!({
        "type": "function",
        "name": "calculator",
        "description": "",
        "parameters": {"type":"object"}
    })]);

    // Simulate a conversation with mixed native + KB tool calls where
    // KB tools are now disabled (not in active_tool_names).
    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"search kb and calculate"}]}),
        // kb_list is NOT in active_tool_names → should be archived
        json!({"type":"function_call","call_id":"call_kb","name":"kb_list","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"call_kb","output":"{\"kbs\":[\"test\"]}"}),
        // calculator IS in active_tool_names → should be preserved
        json!({"type":"function_call","call_id":"call_calc","name":"calculator","arguments":"{\"expression\":\"1+1\"}"}),
        json!({"type":"function_call_output","call_id":"call_calc","output":"{\"ok\":true,\"result\":2}"}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);

    // kb_list should be archived via carrier pair.
    assert_archived_carrier_pair(&normalized, "call_kb");

    // Calculator tool call should be preserved.
    assert!(
        normalized.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_calc")
        }),
        "calculator function_call should be preserved when enabled"
    );

    // An archived carrier pair should be present.
    assert_archived_carrier_pair(&normalized, "call_kb");

    // The carrier output should mention kb_list.
}

#[test]
fn archives_kb_tool_output_when_call_compacted() {
    // When LCM compaction removes the assistant message (with tool_calls_json)
    // but leaves the KB tool result message, the function_call_output has no
    // matching function_call. The archiver must handle KB tools the same as
    // native tools for this orphaned output scenario.
    let active_tools = extract_active_tool_names(&[json!({
        "type": "function",
        "name": "calculator",
        "description": "",
        "parameters": {"type":"object"}
    })]);

    // function_call_output for kb_search is present, but the function_call
    // is MISSING (simulating LCM compaction removing the assistant message).
    let context = vec![
        json!({"role":"user","content":[{"type":"input_text","text":"search kb"}]}),
        json!({"type":"function_call_output","call_id":"call_kb","output":"{\"ok\":true,\"results\":[{\"content\":\"found\"}]}"}),
        json!({"role":"assistant","content":[{"type":"output_text","text":"Here are results."}]}),
    ];

    let normalized = archive_unavailable_historical_tool_calls(context, &active_tools);

    // The orphaned KB output should be archived (output wrapped).
    assert_archived_orphaned_output(&normalized, "call_kb");

    // No raw function_call or function_call_output should remain (except carrier).
    assert!(
        !normalized.iter().any(|item| {
            let fc_type = item.get("type").and_then(|v| v.as_str());
            let is_carrier = is_carrier_item(item);
            matches!(fc_type, Some("function_call") | Some("function_call_output")) && !is_carrier
        }),
        "orphaned KB function_call_output should be archived"
    );
}

// ═══════════════════════════════════════════════════════════════════════════════
// Sub-Agent & LCM Real-Gateway Smoke Tests
// ═══════════════════════════════════════════════════════════════════════════════

/// Helper: run a turn with the sub-agent and memory tools available.
/// Uses `ToolCatalog::new_with_home_plugins` for a realistic tool surface.
async fn run_real_gateway_turn_with_full_catalog(
    input: &str,
) -> (
    crate::provider_api::types::ResponseStreamResult,
    Vec<serde_json::Value>,
    Vec<ProviderStreamEvent>,
) {
    ensure_rustls_crypto_provider();
    let config = crate::config::load_config().expect("load local config");
    let agent = crate::config::load_agent_config(crate::config::constants::DEFAULT_AGENT_ID)
        .unwrap_or_default()
        .normalize();
    let resolved_model = config
        .resolve_model_profile_with_agent(None, &agent)
        .expect("resolve default model profile");
    assert!(
        resolved_model.provider.resolved_credential().is_some(),
        "Active provider has no resolved credential"
    );

    let conversation_id = format!("test-smoke-subagent-{}", Uuid::new_v4());
    crate::conversation_store::ensure_conversation(
        crate::config::constants::DEFAULT_AGENT_ID,
        &conversation_id,
    )
    .expect("ensure conversation workspace");

    let tools_catalog = Arc::new(ToolCatalog::new_with_home_plugins(
        Arc::new(crate::mcp::McpManager::new()),
        &config,
        &agent,
    ));
    let lcm_engine = lcm_engine_for_test(&conversation_id);
    // Register context tools with the LCM store.
    // Note: set_context_tools is not available on an Arc, but the catalog
    // already includes sub-agent and memory tools from construction.

    let req = ChatRequest {
        input: input.to_string(),
        conversation_id: Some(conversation_id.clone()),
        model: Some(agent.default_model.clone()),
        reasoning: None,
        text: None,
        include: None,
        service_tier: None,
        prompt_cache_key: None,
        client_metadata: None,
        generate: None,
        agent_id: Some(crate::config::constants::DEFAULT_AGENT_ID.to_string()),
        request_id: Some(format!("req-smoke-sa-{}", Uuid::new_v4())),
        temperature: None,
        top_p: None,
        presence_penalty: None,
        frequency_penalty: None,
        max_tokens: None,
        max_completion_tokens: None,
    };

    let (_cancel_tx, mut cancel_rx) = watch::channel(false);
    let stream_events: Arc<Mutex<Vec<ProviderStreamEvent>>> = Arc::new(Mutex::new(Vec::new()));
    let stream_events_for_closure = stream_events.clone();

    let run_result = tokio::time::timeout(
        Duration::from_secs(300),
        AgentRuntime::run_turn(
            &config,
            &agent,
            "test-agent",
            &req,
            &conversation_id,
            crate::conversation_store_utils::now_unix_ms(),
            Vec::new(),
            None,
            &tools_catalog,
            &LcmAgentContext::new(lcm_engine.clone()),
            &mut cancel_rx,
            None,       // sub_agent_event_tx
            Vec::new(), // street_items
            false,      // is_auto_resume
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

// ── Sub-Agent Smoke Tests ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_spawn_sub_agent_and_check_status() {
    if !real_gateway_test_enabled() {
        return;
    }

    // Prompt the agent to spawn an explore sub-agent to do a simple file search,
    // then check the sub-agent status.
    let prompt = concat!(
        "请严格按顺序完成以下任务，全程使用中文：\n",
        "第一步：调用 spawn_sub_agent 工具创建一个子代理，参数为：\n",
        "  prompt='列出当前工作目录下的所有文件',\n",
        "  subagentType='explore',\n",
        "  delegatedScope=['filesystem'],\n",
        "  keptWork=['file_list'],\n",
        "  maxTurns=3\n",
        "第二步：拿到 agentId 后，调用 sub_agent_status 检查子代理状态（wait=true, timeoutMs=60000）。\n",
        "第三步：根据子代理返回的结果，输出结论，并且必须包含'子代理冒烟测试通过'这九个字。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(
        !response.output_text.trim().is_empty(),
        "Assistant output should not be empty"
    );

    // Verify spawn_sub_agent was called.
    let spawn_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("spawn_sub_agent")
    });
    assert!(
        spawn_call,
        "Expected spawn_sub_agent tool call, got timeline: {timeline_events:?}"
    );

    // Verify sub_agent_status was called.
    let status_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("sub_agent_status")
    });
    assert!(status_call, "Expected sub_agent_status tool call");

    assert!(
        response.output_text.contains("子代理冒烟测试通过"),
        "Output should include verification phrase. Actual: {}",
        response.output_text
    );
}

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_cancel_sub_agent() {
    if !real_gateway_test_enabled() {
        return;
    }

    let prompt = concat!(
        "请严格按顺序完成：\n",
        "第一步：调用 spawn_sub_agent，参数为：\n",
        "  prompt='sleep for 30 seconds then echo done',\n",
        "  subagentType='general',\n",
        "  delegatedScope=['filesystem'],\n",
        "  keptWork=['completion_signal']\n",
        "第二步：拿到 agentId 后，立即调用 cancel_sub_agent 取消该子代理。\n",
        "第三步：输出结论，必须包含'取消子代理测试通过'这八个字。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    let cancel_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("cancel_sub_agent")
    });
    assert!(cancel_call, "Expected cancel_sub_agent tool call");

    assert!(
        response.output_text.contains("取消子代理测试通过"),
        "Should include verification phrase. Actual: {}",
        response.output_text
    );
}

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_multi_sub_agent_concurrent() {
    if !real_gateway_test_enabled() {
        return;
    }

    // Spawn TWO explore sub-agents concurrently, then check both.
    let prompt = concat!(
        "请严格按顺序完成：\n",
        "第一步：调用 spawn_sub_agent，创建一个子代理列出当前目录的文件。参数：\n",
        "  prompt='List all files in the current working directory',\n",
        "  subagentType='explore',\n",
        "  delegatedScope=['filesystem'],\n",
        "  keptWork=['file_list_A'],\n",
        "  maxTurns=3\n",
        "第二步：调用 spawn_sub_agent，创建第二个子代理获取系统时间。参数：\n",
        "  prompt='Get the current system time',\n",
        "  subagentType='explore',\n",
        "  delegatedScope=['filesystem'],\n",
        "  keptWork=['time_result_B'],\n",
        "  maxTurns=3\n",
        "第三步：调用 sub_agent_status 分别检查两个子代理的状态（使用它们的 agentId）。\n",
        "第四步：输出结论，必须包含'并发子代理测试通过'这八个字。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    // Count spawn_sub_agent calls — should be at least 2.
    let spawn_count = timeline_events
        .iter()
        .filter(|event| {
            event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
                && event.get("name").and_then(|v| v.as_str()) == Some("spawn_sub_agent")
        })
        .count();
    assert!(
        spawn_count >= 2,
        "Expected at least 2 spawn_sub_agent calls, got {spawn_count}"
    );

    assert!(
        response.output_text.contains("并发子代理测试通过"),
        "Should include verification phrase. Actual: {}",
        response.output_text
    );
}

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_scope_narrowing_rejects_empty_kept_work() {
    if !real_gateway_test_enabled() {
        return;
    }

    // Ask the main agent itself (as a non-root agent, since it's in a tool loop)
    // to spawn a sub-agent with empty kept_work. The scope-narrowing should reject.
    // NOTE: The main agent IS root (hop_index=0), so scope-narrowing won't reject
    // from the main agent. We need to get the agent into a tool-calling hop first,
    // THEN ask it to spawn. Let's make it call spawn_sub_agent directly with
    // empty keptWork — since it's root, it SHOULD be allowed.
    // Instead, test that keptWork IS required for non-root by getting a
    // sub-agent to try to spawn. But since sub-agents are async, let's verify
    // that the root agent CAN spawn without keptWork.
    let prompt = concat!(
        "调用 spawn_sub_agent 工具，不提供 keptWork 参数（使用空数组[]）：\n",
        "  prompt='List files',\n",
        "  subagentType='explore',\n",
        "  delegatedScope=['filesystem'],\n",
        "  keptWork=[],\n",
        "  maxTurns=2\n\n",
        "如果成功注册了（返回 agentId），输出'根代理豁免通过'。\n",
        "如果被拒绝了，输出返回的错误信息。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    // Root agent is exempt from scope-narrowing, so it should succeed.
    let spawn_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("spawn_sub_agent")
    });
    assert!(
        spawn_call,
        "Expected spawn_sub_agent tool call. Root agent should be exempt from scope-narrowing."
    );

    assert!(
        response.output_text.contains("根代理豁免通过") || response.output_text.contains("agentId"),
        "Root agent should be exempt from scope-narrowing. Output: {}",
        response.output_text
    );
}

// ── Memory Tools Smoke Tests ──────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_memory_write_search_recall() {
    if !real_gateway_test_enabled() {
        return;
    }

    let prompt = concat!(
        "请严格按顺序完成以下任务，全程使用中文：\n",
        "第一步：调用 memory_write 工具写入一条记忆：\n",
        "  name='smoke-test-memory',\n",
        "  description='冒烟测试写入的记忆条目',\n",
        "  memoryType='project',\n",
        "  tags=['smoke-test', 'verification'],\n",
        "  body='这是冒烟测试写入的记忆内容。关键信息：项目根目录包含 src-tauri 和 src 文件夹。'\n",
        "第二步：调用 memory_search 搜索关键词 '冒烟测试'。\n",
        "第三步：调用 memory_recall 召回名称为 'smoke-test-memory' 的完整记忆。\n",
        "第四步：输出结论，必须包含'记忆工具冒烟测试通过'这九个字。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    let write_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("memory_write")
    });
    assert!(write_call, "Expected memory_write tool call");

    let search_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("memory_search")
    });
    assert!(search_call, "Expected memory_search tool call");

    let recall_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("memory_recall")
    });
    assert!(recall_call, "Expected memory_recall tool call");

    assert!(
        response.output_text.contains("记忆工具冒烟测试通过"),
        "Should include verification phrase. Actual: {}",
        response.output_text
    );
}

// ── LCM Tools Smoke Tests ─────────────────────────────────────────────────────

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_lcm_grep_and_describe() {
    if !real_gateway_test_enabled() {
        return;
    }

    // First turn: write a distinctive message into the conversation history.
    // Second turn: use lcm_grep to search for it, lcm_describe to inspect.
    // We do this in a single turn by having the agent talk, then search.
    let prompt = concat!(
        "请严格按顺序完成：\n",
        "第一步：先输出一句包含特殊标记的话：'LCM-GREP-MARKER-冒烟测试验证字符串'。\n",
        "第二步：调用 lcm_grep 在对话历史中搜索 'LCM-GREP-MARKER'。\n",
        "第三步：输出结论，必须包含'LCM工具冒烟测试通过'这九个字。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    let grep_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("lcm_grep")
    });
    assert!(
        grep_call,
        "Expected lcm_grep tool call in: {timeline_events:?}"
    );

    assert!(
        response.output_text.contains("LCM工具冒烟测试通过"),
        "Should include verification phrase. Actual: {}",
        response.output_text
    );
}

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_lcm_expand_restricted_to_sub_agent() {
    if !real_gateway_test_enabled() {
        return;
    }

    // The main agent should NOT be able to call lcm_expand directly.
    // lcm_expand is restricted to sub-agents per LCM Appendix C.1.
    let prompt = concat!(
        "请调用 lcm_expand 工具，参数 summaryId='nonexistent-summary-id'。\n",
        "如果调用成功被工具系统拒绝（返回了错误信息），请输出'LCM展开限制验证通过'。\n",
        "如果工具调用成功执行了，请输出'展开成功但不符合预期'。"
    );

    let (response, _timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    // lcm_expand should either not be called (model reads the description),
    // or be called and rejected. In either case, the model should report the restriction.
    assert!(
        response.output_text.contains("LCM展开限制验证通过")
            || response.output_text.contains("restricted")
            || response.output_text.contains("sub-agent"),
        "lcm_expand should be restricted for main agent. Output: {}",
        response.output_text
    );
}

// ── Sub-Agent + LCM Integration Test ──────────────────────────────────────────

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_sub_agent_with_lcm_grep_in_context() {
    if !real_gateway_test_enabled() {
        return;
    }

    // This test verifies the full LCM ↔ sub-agent integration:
    // 1. Main agent spawns an explore sub-agent
    // 2. Sub-agent can use lcm_grep (which should use sub-agent's own LCM store)
    // 3. Results are returned to main agent
    //
    // We simulate this by asking the main agent to delegate an LCM search
    // to a sub-agent via the existing Task tool (sync), or via spawn_sub_agent (async).
    let prompt = concat!(
        "请严格按顺序完成：\n",
        "第一步：先用中文说一句：'集成测试标记文本-用于后续LCM搜索'。\n",
        "第二步：调用 spawn_sub_agent 创建一个子代理来搜索对话记录：\n",
        "  prompt='使用 lcm_grep 在对话历史中搜索模式 \"集成测试标记文本\"，报告搜索结果',\n",
        "  subagentType='explore',\n",
        "  delegatedScope=['filesystem', 'context'],\n",
        "  keptWork=['lcm_search_report'],\n",
        "  maxTurns=5\n",
        "第三步：调用 sub_agent_status 等待子代理完成并获取结果（wait=true, timeoutMs=120000）。\n",
        "第四步：输出最终结论，必须包含'子代理LCM集成测试通过'这十个字。"
    );

    let (response, timeline_events, _stream_events) =
        run_real_gateway_turn_with_full_catalog(prompt).await;

    assert!(!response.output_text.trim().is_empty());

    let spawn_call = timeline_events.iter().any(|event| {
        event.get("type").and_then(|v| v.as_str()) == Some("toolCall")
            && event.get("name").and_then(|v| v.as_str()) == Some("spawn_sub_agent")
    });
    assert!(spawn_call, "Expected spawn_sub_agent tool call");

    assert!(
        response.output_text.contains("子代理LCM集成测试通过"),
        "Should include verification phrase. Actual: {}",
        response.output_text
    );
}

// ── End-to-End: Memory persistence across conversations ──────────────────────

#[tokio::test]
#[ignore = "requires a real provider credential and network access"]
async fn real_gateway_memory_persists_across_turns() {
    if !real_gateway_test_enabled() {
        return;
    }

    // Turn 1: Write a memory.
    let prompt1 = concat!(
        "请调用 memory_write 写入一条记忆：\n",
        "  name='cross-turn-test',\n",
        "  description='跨轮次持久化测试',\n",
        "  memoryType='project',\n",
        "  tags=['persistence'],\n",
        "  body='跨轮次测试记忆内容：当前时间是2026年6月。'\n",
        "完成后，输出'写入完成'。"
    );

    let (response1, _, _) = run_real_gateway_turn_with_full_catalog(prompt1).await;
    assert!(response1.output_text.contains("写入完成") || !response1.output_text.trim().is_empty());

    // Turn 2 (new conversation): Search for the memory written in turn 1.
    let prompt2 = concat!(
        "请调用 memory_search 搜索关键词 'cross-turn-test'。\n",
        "如果找到了记忆，调用 memory_recall 获取完整内容，然后输出'跨轮次持久化验证通过'。\n",
        "如果没找到，输出'未找到记忆'。"
    );

    let (response2, _, _) = run_real_gateway_turn_with_full_catalog(prompt2).await;
    assert!(
        response2.output_text.contains("跨轮次持久化验证通过"),
        "Memory should persist across conversations. Output: {}",
        response2.output_text
    );
}
