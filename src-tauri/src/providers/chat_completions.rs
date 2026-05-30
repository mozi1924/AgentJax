mod payload;
mod stream;

use std::collections::BTreeMap;
use std::time::Duration;

use futures_util::StreamExt;
use serde_json::{Value, json};
use tokio::sync::watch;

use super::core::ProviderIdFactory;
use super::registry;
use super::responses;
use super::sse::split_sse_event_block;
use super::types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ProviderUsage,
    ProviderUsageRecord, ResponseStreamRequest, ResponseStreamResult,
};
use crate::config::ResolvedModelConfig;
use payload::build_chat_completions_payload;
use stream::{
    ChatToolCallAccumulator, finalize_pending_tool_calls, process_chat_completions_event,
};

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let strategy = responses::models::ModelsFetchStrategy::openai_compatible()
        .with_provider_overrides(&resolved.provider.models_endpoint_candidates);
    responses::models::fetch_remote_models_with_strategy(resolved, &strategy).await
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential();
    let request_headers = responses::http::merge_request_headers(
        &[("Content-Type", "application/json")],
        &resolved.provider,
        None,
        credential.as_deref(),
    );
    let endpoint = format!(
        "{}/chat/completions",
        resolved.provider.api_endpoint.trim_end_matches('/')
    );
    let endpoint =
        responses::http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params)
            .map_err(|e| format!("Failed to build Chat Completions endpoint URL: {e}"))?;
    let body = build_chat_completions_payload(resolved, req)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request = responses::http::apply_headers_to_reqwest(
        client.post(endpoint).json(&body),
        &request_headers,
    )
    .map_err(|e| format!("Failed to prepare Chat Completions request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Chat Completions API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Chat Completions API error ({status}): {text}"));
    }

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut usage: Option<ProviderUsage> = None;
    let mut emitted_output_started = false;
    let mut id_factory = ProviderIdFactory::new("chat_completions");
    let mut tool_calls_by_index: BTreeMap<usize, ChatToolCallAccumulator> = BTreeMap::new();
    let mut output_items: Vec<Value> = Vec::new();
    let mut buffer = String::new();
    let mut stream = response.bytes_stream();

    loop {
        tokio::select! {
            changed = cancel_rx.changed() => {
                if changed.is_ok() && *cancel_rx.borrow() {
                    break;
                }
            }
            next_chunk = stream.next() => {
                let Some(next_chunk) = next_chunk else {
                    break;
                };
                let bytes = next_chunk
                    .map_err(|e| format!("Failed to read Chat Completions stream: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));

                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    process_chat_completions_event(
                        &event_block,
                        &mut response_id,
                        &mut output_text,
                        &mut usage,
                        &mut emitted_output_started,
                        &mut id_factory,
                        &mut tool_calls_by_index,
                        &mut output_items,
                        on_delta,
                    )?;
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        process_chat_completions_event(
            &buffer,
            &mut response_id,
            &mut output_text,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_calls_by_index,
            &mut output_items,
            on_delta,
        )?;
    }

    finalize_pending_tool_calls(&mut tool_calls_by_index, &mut output_items, on_delta)?;

    if !output_text.trim().is_empty() {
        output_items.insert(
            0,
            json!({
                "type": "message",
                "role": "assistant",
                "content": [{
                    "type": "output_text",
                    "text": output_text
                }]
            }),
        );
    }

    if response_id.is_empty() {
        response_id = id_factory.response_id().to_string();
    }

    let usage_hops: Vec<ProviderUsageRecord> = usage
        .clone()
        .map(|usage| ProviderUsageRecord {
            response_id: response_id.clone(),
            usage,
        })
        .into_iter()
        .collect();

    Ok(ResponseStreamResult {
        response_id,
        output_text,
        output_items,
        usage,
        usage_hops,
        provider_key: resolved.provider_key.clone(),
        model_profile: resolved.profile_key.clone(),
        model_id: resolved.model_id.clone(),
        capabilities: registry::provider_capabilities("chat-completions")
            .unwrap_or_else(|| registry::default_provider_definition().capabilities),
    })
}

pub fn get_reasoning_capability(
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> ModelReasoningCapability {
    let supported_reasoning_levels = cached_levels
        .map(responses::normalize_reasoning_levels)
        .filter(|levels| !levels.is_empty())
        .unwrap_or_else(|| responses::infer_reasoning_levels_from_model_id(model_id));

    ModelReasoningCapability {
        supports_reasoning: !supported_reasoning_levels.is_empty(),
        supported_reasoning_levels,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        ChatToolCallAccumulator, build_chat_completions_payload, process_chat_completions_event,
    };
    use crate::config::{
        ModelRequestConfig, PromptComposerConfig, ProviderConfig, ResolvedModelConfig,
        compile_prompt_composer,
    };
    use crate::providers::core::ProviderIdFactory;
    use crate::providers::types::{ProviderStreamEvent, ResponseStreamRequest};
    use serde_json::{Value, json};
    use std::collections::BTreeMap;

    fn test_resolved() -> ResolvedModelConfig {
        let prompt_assembly = compile_prompt_composer(&PromptComposerConfig::default());
        ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "chat-completions".to_string(),
            provider: ProviderConfig::default(),
            model_id: "gpt-4o".to_string(),
            model_ref: "chat-completions/gpt-4o".to_string(),
            system_prompt: "system prompt".to_string(),
            prompt_assembly,
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        }
    }

    #[test]
    fn payload_converts_responses_items_to_chat_messages() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![
                json!({
                    "role": "user",
                    "content": [{"type":"input_text","text":"hello"}]
                }),
                json!({
                    "type": "function_call",
                    "call_id": "call_1",
                    "name": "lookup",
                    "arguments": {"q": "agent"}
                }),
                json!({
                    "type": "function_call_output",
                    "call_id": "call_1",
                    "output": "ok"
                }),
            ],
            model: Some("gpt-4o".to_string()),
            reasoning_effort: None,
            instructions_override: None,
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: None,
            tool_choice: None,
        };

        let payload = build_chat_completions_payload(&resolved, &req).unwrap();
        let messages = payload.get("messages").and_then(Value::as_array).unwrap();
        assert_eq!(messages.len(), 4);
        assert_eq!(
            messages[0].get("role").and_then(Value::as_str),
            Some("system")
        );
        assert_eq!(
            messages[2]
                .get("tool_calls")
                .and_then(Value::as_array)
                .and_then(|calls| calls.first())
                .and_then(|call| call.get("id"))
                .and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            messages[3].get("tool_call_id").and_then(Value::as_str),
            Some("call_1")
        );
        assert_eq!(
            payload
                .get("stream_options")
                .and_then(|v| v.get("include_usage"))
                .and_then(Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn stream_parser_normalizes_tool_calls_and_usage() {
        let mut response_id = String::new();
        let mut output_text = String::new();
        let mut usage = None;
        let mut emitted_output_started = false;
        let mut id_factory = ProviderIdFactory::new("chat_completions");
        let mut tool_calls_by_index: BTreeMap<usize, ChatToolCallAccumulator> = BTreeMap::new();
        let mut output_items = Vec::new();
        let mut events = Vec::new();

        let first = r#"data: {"id":"chatcmpl_1","choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"lookup","arguments":"{\"q\""}}]},"finish_reason":null}]}"#;
        let second = r#"data: {"id":"chatcmpl_1","choices":[{"delta":{"tool_calls":[{"index":0,"function":{"arguments":":\"agent\"}"}}]},"finish_reason":"tool_calls"}],"usage":{"prompt_tokens":10,"completion_tokens":4,"total_tokens":14}}"#;

        process_chat_completions_event(
            first,
            &mut response_id,
            &mut output_text,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_calls_by_index,
            &mut output_items,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();
        process_chat_completions_event(
            second,
            &mut response_id,
            &mut output_text,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_calls_by_index,
            &mut output_items,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(response_id, "chatcmpl_1");
        assert_eq!(usage.unwrap().total_tokens, 14);
        assert_eq!(output_items.len(), 1);
        assert_eq!(
            output_items[0].get("call_id").and_then(Value::as_str),
            Some("call_1")
        );
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ProviderStreamEvent::ToolCallCompleted { call_id, .. } if call_id == "call_1"
            )
        }));
    }
}
