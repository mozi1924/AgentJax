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
use payload::build_anthropic_payload;
use stream::{AnthropicToolBlock, finalize_all_tool_blocks, process_anthropic_event};

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let endpoint = format!(
        "{}/models",
        resolved.provider.api_endpoint().trim_end_matches('/')
    );
    let endpoint =
        responses::http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params())
            .map_err(|e| format!("Failed to build Anthropic models endpoint URL: {e}"))?;
    let headers =
        build_request_headers(resolved, resolved.provider.resolved_credential().as_deref());
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request = responses::http::apply_headers_to_reqwest(client.get(endpoint), &headers)
        .map_err(|e| format!("Failed to prepare Anthropic models request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Anthropic models API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Anthropic models API error ({status}): {text}"));
    }

    let root: Value = response
        .json()
        .await
        .map_err(|e| format!("Failed to parse Anthropic model list JSON: {e}"))?;
    let models = parse_model_descriptors(&root);
    if models.is_empty() {
        return Err("Anthropic model list did not contain recognized model ids".to_string());
    }
    Ok(models)
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let credential = resolved.provider.resolved_credential();
    let headers = build_request_headers(resolved, credential.as_deref());
    let endpoint = format!(
        "{}/messages",
        resolved.provider.api_endpoint().trim_end_matches('/')
    );
    let endpoint =
        responses::http::apply_query_params_to_url(&endpoint, &resolved.provider.query_params())
            .map_err(|e| format!("Failed to build Anthropic messages endpoint URL: {e}"))?;
    let body = build_anthropic_payload(resolved, req)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request =
        responses::http::apply_headers_to_reqwest(client.post(endpoint).json(&body), &headers)
            .map_err(|e| format!("Failed to prepare Anthropic request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Anthropic API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Anthropic API error ({status}): {text}"));
    }

    let mut response_id = String::new();
    let mut output_text = String::new();
    let mut output_items = Vec::new();
    let mut usage: Option<ProviderUsage> = None;
    let mut emitted_output_started = false;
    let mut id_factory = ProviderIdFactory::new("anthropic");
    let mut tool_blocks_by_index: BTreeMap<usize, AnthropicToolBlock> = BTreeMap::new();
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
                let bytes = next_chunk.map_err(|e| format!("Failed to read Anthropic stream: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    process_anthropic_event(
                        &event_block,
                        &mut response_id,
                        &mut output_text,
                        &mut output_items,
                        &mut usage,
                        &mut emitted_output_started,
                        &mut id_factory,
                        &mut tool_blocks_by_index,
                        on_delta,
                    )?;
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        process_anthropic_event(
            &buffer,
            &mut response_id,
            &mut output_text,
            &mut output_items,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut tool_blocks_by_index,
            on_delta,
        )?;
    }
    finalize_all_tool_blocks(&mut tool_blocks_by_index, &mut output_items, on_delta)?;

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
        capabilities: registry::provider_capabilities("anthropic")
            .unwrap_or_else(|| registry::default_provider_definition().capabilities),
    })
}

pub fn get_reasoning_capability(
    _model_id: &str,
    cached_levels: Option<&[String]>,
) -> ModelReasoningCapability {
    let supported_reasoning_levels = cached_levels
        .map(responses::normalize_reasoning_levels)
        .filter(|levels| !levels.is_empty())
        .unwrap_or_default();

    ModelReasoningCapability {
        supports_reasoning: !supported_reasoning_levels.is_empty(),
        supported_reasoning_levels,
    }
}

fn build_request_headers(
    resolved: &ResolvedModelConfig,
    credential: Option<&str>,
) -> BTreeMap<String, String> {
    let mut headers = responses::http::merge_request_headers(
        &[
            ("Content-Type", "application/json"),
            ("anthropic-version", "2023-06-01"),
        ],
        &resolved.provider,
        None,
        None,
    );

    if !has_header_case_insensitive(&headers, "x-api-key")
        && !has_header_case_insensitive(&headers, "Authorization")
    {
        if let Some(credential) = credential.map(str::trim).filter(|value| !value.is_empty()) {
            headers.insert("x-api-key".to_string(), credential.to_string());
        }
    }

    headers
}

fn has_header_case_insensitive(headers: &BTreeMap<String, String>, name: &str) -> bool {
    headers.keys().any(|key| key.eq_ignore_ascii_case(name))
}

fn parse_model_descriptors(root: &Value) -> Vec<ProviderModelDescriptor> {
    let mut out = Vec::new();
    if let Some(items) = root.get("data").and_then(Value::as_array) {
        for item in items {
            let id = item
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| item.get("model").and_then(Value::as_str))
                .unwrap_or("")
                .trim();
            if !id.is_empty() {
                out.push(ProviderModelDescriptor {
                    id: id.to_string(),
                    supported_reasoning_levels: Vec::new(),
                });
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{build_anthropic_payload, process_anthropic_event};
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
            provider_key: "anthropic".to_string(),
            provider: ProviderConfig::default(),
            model_id: "claude-sonnet-4-5".to_string(),
            model_ref: "anthropic/claude-sonnet-4-5".to_string(),
            system_prompt: "system prompt".to_string(),
            prompt_assembly,
            request: ModelRequestConfig {
                max_output_tokens: Some(1024),
                ..Default::default()
            },
            timeout_seconds: 60,
        }
    }

    #[test]
    fn payload_converts_timeline_to_anthropic_messages() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![
                json!({
                    "role": "developer",
                    "content": [{"type":"input_text","text":"dev note"}]
                }),
                json!({
                    "role": "user",
                    "content": [{"type":"input_text","text":"hello"}]
                }),
                json!({
                    "type":"function_call",
                    "call_id":"toolu_1",
                    "name":"lookup",
                    "arguments":{"q":"agent"}
                }),
                json!({
                    "type":"function_call_output",
                    "call_id":"toolu_1",
                    "output":"ok"
                }),
            ],
            model: Some("claude-sonnet-4-5".to_string()),
            reasoning_effort: None,
            instructions_override: None,
            text: None,
            include: None,
            service_tier: None,
            prompt_cache_key: None,
            client_metadata: None,
            generate: None,
            tools: Some(vec![json!({
                "name": "lookup",
                "description": "Lookup",
                "input_schema": {"type":"object"}
            })]),
            tool_choice: None,
        };

        let payload = build_anthropic_payload(&resolved, &req).unwrap();
        assert_eq!(
            payload.get("model").and_then(Value::as_str),
            Some("claude-sonnet-4-5")
        );
        assert_eq!(
            payload.get("max_tokens").and_then(Value::as_u64),
            Some(1024)
        );
        assert!(
            payload
                .get("system")
                .and_then(Value::as_str)
                .unwrap_or("")
                .contains("dev note")
        );
        let messages = payload.get("messages").and_then(Value::as_array).unwrap();
        assert_eq!(messages.len(), 3);
        assert!(
            messages[1]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("tool_use")
        );
        assert!(
            messages[2]
                .get("content")
                .and_then(Value::as_array)
                .and_then(|content| content.first())
                .and_then(|part| part.get("type"))
                .and_then(Value::as_str)
                == Some("tool_result")
        );
    }

    #[test]
    fn payload_normalizes_tool_choice_and_omits_openai_response_format() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![json!({
                "role": "user",
                "content": [{"type":"input_text","text":"reply as JSON"}]
            })],
            model: Some("claude-sonnet-4-5".to_string()),
            text: Some(json!({ "format": { "type": "json_object" } })),
            tools: Some(vec![json!({
                "name": "lookup",
                "description": "Lookup",
                "input_schema": {"type":"object"}
            })]),
            tool_choice: Some(Value::String("auto".to_string())),
            ..Default::default()
        };

        let payload = build_anthropic_payload(&resolved, &req).unwrap();
        assert_eq!(
            payload
                .get("tool_choice")
                .and_then(|choice| choice.get("type"))
                .and_then(Value::as_str),
            Some("auto")
        );
        assert!(payload.get("response_format").is_none());
    }

    #[test]
    fn payload_omits_tool_choice_when_no_tools_are_sent() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![json!({
                "role": "user",
                "content": [{"type":"input_text","text":"hello"}]
            })],
            model: Some("claude-sonnet-4-5".to_string()),
            tool_choice: Some(Value::String("auto".to_string())),
            ..Default::default()
        };

        let payload = build_anthropic_payload(&resolved, &req).unwrap();
        assert!(payload.get("tools").is_none());
        assert!(payload.get("tool_choice").is_none());
    }

    #[test]
    fn stream_parser_normalizes_tool_use_and_usage() {
        let mut response_id = String::new();
        let mut output_text = String::new();
        let mut output_items = Vec::new();
        let mut usage = None;
        let mut emitted_output_started = false;
        let mut id_factory = ProviderIdFactory::new("anthropic");
        let mut tool_blocks = BTreeMap::new();
        let mut events = Vec::new();

        let events_raw = [
            r#"data: {"type":"message_start","message":{"id":"msg_1","usage":{"input_tokens":10,"cache_read_input_tokens":2,"output_tokens":1}}}"#,
            r#"data: {"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"hello"}}"#,
            r#"data: {"type":"content_block_start","index":1,"content_block":{"type":"tool_use","id":"toolu_1","name":"lookup","input":{}}}"#,
            r#"data: {"type":"content_block_delta","index":1,"delta":{"type":"input_json_delta","partial_json":"{\"q\":\"agent\"}"}}"#,
            r#"data: {"type":"content_block_stop","index":1}"#,
            r#"data: {"type":"message_delta","usage":{"output_tokens":7}}"#,
        ];

        for raw in events_raw {
            process_anthropic_event(
                raw,
                &mut response_id,
                &mut output_text,
                &mut output_items,
                &mut usage,
                &mut emitted_output_started,
                &mut id_factory,
                &mut tool_blocks,
                &mut |event| {
                    events.push(event);
                    Ok(())
                },
            )
            .unwrap();
        }

        let usage = usage.unwrap();
        assert_eq!(response_id, "msg_1");
        assert_eq!(output_text, "hello");
        assert_eq!(usage.prompt_tokens, 12);
        assert_eq!(usage.completion_tokens, 7);
        assert_eq!(usage.total_tokens, 19);
        assert!(output_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some("toolu_1")
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ProviderStreamEvent::ToolCallCompleted { call_id, .. } if call_id == "toolu_1"
            )
        }));
    }
}
