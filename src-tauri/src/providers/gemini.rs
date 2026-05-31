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
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ProviderUsageRecord,
    ResponseStreamRequest, ResponseStreamResult,
};
use crate::config::ResolvedModelConfig;
use payload::build_gemini_payload;
use stream::process_gemini_event;

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let strategy = models_fetch_strategy(resolved);
    responses::models::fetch_remote_models_with_strategy(resolved, &strategy).await
}

fn models_fetch_strategy(resolved: &ResolvedModelConfig) -> responses::models::ModelsFetchStrategy {
    let mut strategy = responses::models::ModelsFetchStrategy::openai_compatible()
        .with_provider_overrides(&resolved.provider.models_endpoint_candidates());

    // Google Gemini's public REST API accepts API keys as a `key` query
    // parameter for both generation and model catalog endpoints.
    if should_use_key_query_param(&resolved.provider.api_endpoint()) {
        strategy = strategy.with_credential_query_param("key");
    }

    strategy
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    let mut id_factory = ProviderIdFactory::new("gemini");
    let credential = resolved.provider.resolved_credential();
    let endpoint = build_stream_endpoint(resolved, credential.as_deref())?;
    let headers = build_request_headers(resolved, credential.as_deref(), &endpoint);
    let body = build_gemini_payload(resolved, req)?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let request = responses::http::apply_headers_to_reqwest(
        client.post(endpoint.clone()).json(&body),
        &headers,
    )
    .map_err(|e| format!("Failed to prepare Gemini request headers: {e}"))?;
    let response = request
        .send()
        .await
        .map_err(|err| format!("Failed to reach Gemini API: {err}"))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(format!("Gemini API error ({status}): {text}"));
    }

    let response_id = id_factory.response_id().to_string();
    let mut output_text = String::new();
    let mut output_items = Vec::new();
    let mut usage = None;
    let mut emitted_output_started = false;
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
                let bytes = next_chunk.map_err(|e| format!("Failed to read Gemini stream: {e}"))?;
                buffer.push_str(&String::from_utf8_lossy(&bytes));
                while let Some((event_block, rest)) = split_sse_event_block(&buffer) {
                    buffer = rest;
                    process_gemini_event(
                        &event_block,
                        &response_id,
                        &mut output_text,
                        &mut output_items,
                        &mut usage,
                        &mut emitted_output_started,
                        &mut id_factory,
                        on_delta,
                    )?;
                }
            }
        }
    }

    if !buffer.trim().is_empty() {
        process_gemini_event(
            &buffer,
            &response_id,
            &mut output_text,
            &mut output_items,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            on_delta,
        )?;
    }

    if !output_text.trim().is_empty()
        && !output_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
    {
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
        capabilities: registry::provider_capabilities("gemini")
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

fn build_stream_endpoint(
    resolved: &ResolvedModelConfig,
    credential: Option<&str>,
) -> Result<String, String> {
    let model = resolved.model_id.trim().trim_start_matches("models/");
    let base = resolved.provider.api_endpoint();
    let base = base.trim_end_matches('/');
    let endpoint = format!("{base}/models/{model}:streamGenerateContent");
    let mut query_params = resolved.provider.query_params();
    query_params
        .entry("alt".to_string())
        .or_insert_with(|| "sse".to_string());

    if should_use_key_query_param(base) {
        if let Some(credential) = credential.map(str::trim).filter(|value| !value.is_empty()) {
            query_params
                .entry("key".to_string())
                .or_insert_with(|| credential.to_string());
        }
    }

    responses::http::apply_query_params_to_url(&endpoint, &query_params)
        .map_err(|e| format!("Failed to build Gemini endpoint URL: {e}"))
}

fn should_use_key_query_param(endpoint: &str) -> bool {
    endpoint.contains("generativelanguage.googleapis.com")
}

fn build_request_headers(
    resolved: &ResolvedModelConfig,
    credential: Option<&str>,
    endpoint: &str,
) -> BTreeMap<String, String> {
    let bearer_credential = (!should_use_key_query_param(endpoint))
        .then_some(credential)
        .flatten();
    responses::http::merge_request_headers(
        &[("Content-Type", "application/json")],
        &resolved.provider,
        None,
        bearer_credential,
    )
}

#[cfg(test)]
mod tests {
    use super::{build_gemini_payload, models_fetch_strategy, process_gemini_event};
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
        let mut custom_settings = serde_json::Map::new();
        custom_settings.insert(
            "apiEndpoint".to_string(),
            serde_json::Value::String(
                "https://generativelanguage.googleapis.com/v1beta".to_string(),
            ),
        );
        let provider = ProviderConfig {
            kind: "gemini".to_string(),
            models: BTreeMap::new(),
            custom_settings,
        };
        ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "gemini".to_string(),
            provider,
            model_id: "gemini-2.5-flash".to_string(),
            model_ref: "gemini/gemini-2.5-flash".to_string(),
            system_prompt: "system prompt".to_string(),
            prompt_assembly,
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        }
    }

    #[test]
    fn models_strategy_uses_key_query_for_google_endpoint() {
        let resolved = test_resolved();
        let strategy = models_fetch_strategy(&resolved);

        assert_eq!(strategy.credential_query_param, Some("key"));
    }

    #[test]
    fn payload_converts_timeline_to_gemini_contents() {
        let resolved = test_resolved();
        let req = ResponseStreamRequest {
            input_items: vec![
                json!({
                    "role": "user",
                    "content": [{"type":"input_text","text":"hello"}]
                }),
                json!({
                    "type":"function_call",
                    "call_id":"call_1",
                    "name":"lookup",
                    "arguments":"{\"q\":\"agent\"}"
                }),
                json!({
                    "type":"function_call_output",
                    "call_id":"call_1",
                    "output":"{\"ok\":true}"
                }),
            ],
            model: Some("gemini-2.5-flash".to_string()),
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
                "parameters": {"type":"object"}
            })]),
            tool_choice: None,
        };

        let payload = build_gemini_payload(&resolved, &req).unwrap();
        let contents = payload.get("contents").and_then(Value::as_array).unwrap();
        assert_eq!(contents.len(), 3);
        assert!(
            contents[1]
                .get("parts")
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|part| part.get("functionCall"))
                .is_some()
        );
        assert!(
            contents[2]
                .get("parts")
                .and_then(Value::as_array)
                .and_then(|parts| parts.first())
                .and_then(|part| part.get("functionResponse"))
                .is_some()
        );
        assert!(
            payload
                .get("tools")
                .and_then(Value::as_array)
                .and_then(|tools| tools.first())
                .and_then(|tool| tool.get("functionDeclarations"))
                .is_some()
        );
    }

    #[test]
    fn stream_parser_normalizes_function_calls_and_usage() {
        let mut output_text = String::new();
        let mut output_items = Vec::new();
        let mut usage = None;
        let mut emitted_output_started = false;
        let mut id_factory = ProviderIdFactory::new("gemini");
        let mut events = Vec::new();
        let payload = r#"data: {"candidates":[{"content":{"parts":[{"text":"hello"},{"functionCall":{"name":"lookup","args":{"q":"agent"}}}]}}],"usageMetadata":{"promptTokenCount":12,"candidatesTokenCount":8,"totalTokenCount":20}}"#;

        process_gemini_event(
            payload,
            "resp_1",
            &mut output_text,
            &mut output_items,
            &mut usage,
            &mut emitted_output_started,
            &mut id_factory,
            &mut |event| {
                events.push(event);
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(output_text, "hello");
        assert_eq!(usage.unwrap().total_tokens, 20);
        assert!(output_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("name").and_then(Value::as_str) == Some("lookup")
        }));
        assert!(events.iter().any(|event| {
            matches!(
                event,
                ProviderStreamEvent::ToolCallCompleted { name, .. } if name == "lookup"
            )
        }));
    }
}
