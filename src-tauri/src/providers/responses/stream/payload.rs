use serde_json::{json, Value};

use crate::config::{ModelRequestConfig, ResolvedModelConfig};
use crate::providers::types::ResponseStreamRequest;

pub(crate) fn build_streaming_request_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    store: bool,
    include_stream_field: bool,
) -> Value {
    let input_items = normalize_input_items_for_responses(&req.input_items);

    let mut payload = json!({
      "model": resolved.model_id,
      "instructions": req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt),
      "input": input_items,
      "store": store
    });

    if include_stream_field {
        payload["stream"] = Value::Bool(true);
    }

    apply_model_request_config(
        &mut payload,
        &resolved.request,
        req.reasoning_effort.as_deref(),
    );

    if let Some(tools) = &req.tools {
        if !tools.is_empty() {
            payload["tools"] = json!(tools);
        }
    }
    if let Some(tool_choice) = &req.tool_choice {
        payload["tool_choice"] = tool_choice.clone();
    }
    if let Some(text) = &req.text {
        payload["text"] = text.clone();
    }
    if let Some(include) = req.include.as_ref().filter(|items| !items.is_empty()) {
        payload["include"] = json!(include);
    }
    if let Some(service_tier) = req
        .service_tier
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload["service_tier"] = Value::String(service_tier.to_string());
    }
    if let Some(prompt_cache_key) = req
        .prompt_cache_key
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        payload["prompt_cache_key"] = Value::String(prompt_cache_key.to_string());
    }
    if let Some(client_metadata) = req
        .client_metadata
        .as_ref()
        .filter(|value| value.is_object())
    {
        payload["client_metadata"] = client_metadata.clone();
    }
    if let Some(generate) = req.generate {
        payload["generate"] = Value::Bool(generate);
    }

    payload
}

#[cfg(test)]
mod tests {
    use super::build_streaming_request_payload;
    use crate::config::{ModelRequestConfig, ProviderConfig, ResolvedModelConfig};
    use crate::providers::types::ResponseStreamRequest;

    #[test]
    fn websocket_payload_can_omit_stream_field() {
        let resolved = ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "openai-responses".to_string(),
            provider: ProviderConfig::default(),
            model_id: "gpt-5-mini".to_string(),
            model_ref: "openai-responses/gpt-5-mini".to_string(),
            system_prompt: "test prompt".to_string(),
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        };
        let req = ResponseStreamRequest {
            input_items: Vec::new(),
            model: Some("gpt-5-mini".to_string()),
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

        let payload = build_streaming_request_payload(&resolved, &req, false, false);
        assert!(payload.get("stream").is_none());
    }

    #[test]
    fn sse_payload_includes_stream_field() {
        let resolved = ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "openai-responses".to_string(),
            provider: ProviderConfig::default(),
            model_id: "gpt-5-mini".to_string(),
            model_ref: "openai-responses/gpt-5-mini".to_string(),
            system_prompt: "test prompt".to_string(),
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        };
        let req = ResponseStreamRequest {
            input_items: Vec::new(),
            model: Some("gpt-5-mini".to_string()),
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

        let payload = build_streaming_request_payload(&resolved, &req, false, true);
        assert_eq!(payload.get("stream").and_then(|v| v.as_bool()), Some(true));
        assert!(payload.get("previous_response_id").is_none());
    }

    #[test]
    fn payload_includes_optional_compatibility_fields() {
        let resolved = ResolvedModelConfig {
            profile_key: "test".to_string(),
            provider_key: "openai-responses".to_string(),
            provider: ProviderConfig::default(),
            model_id: "gpt-5-mini".to_string(),
            model_ref: "openai-responses/gpt-5-mini".to_string(),
            system_prompt: "test prompt".to_string(),
            request: ModelRequestConfig::default(),
            timeout_seconds: 60,
        };
        let req = ResponseStreamRequest {
            input_items: Vec::new(),
            model: Some("gpt-5-mini".to_string()),
            reasoning_effort: None,
            instructions_override: None,
            text: Some(serde_json::json!({ "format": { "type": "text" } })),
            include: Some(vec!["reasoning.encrypted_content".to_string()]),
            service_tier: Some("flex".to_string()),
            prompt_cache_key: Some("conversation-1".to_string()),
            client_metadata: Some(serde_json::json!({ "app": "agentjax" })),
            generate: Some(false),
            tools: None,
            tool_choice: None,
        };

        let payload = build_streaming_request_payload(&resolved, &req, false, false);
        assert!(payload.get("text").is_some());
        assert_eq!(
            payload
                .get("include")
                .and_then(|v| v.as_array())
                .map(std::vec::Vec::len),
            Some(1)
        );
        assert_eq!(
            payload.get("service_tier").and_then(|v| v.as_str()),
            Some("flex")
        );
        assert_eq!(
            payload.get("prompt_cache_key").and_then(|v| v.as_str()),
            Some("conversation-1")
        );
        assert_eq!(
            payload
                .get("client_metadata")
                .and_then(|v| v.get("app"))
                .and_then(|v| v.as_str()),
            Some("agentjax")
        );
        assert_eq!(
            payload.get("generate").and_then(|v| v.as_bool()),
            Some(false)
        );
    }
}

fn apply_model_request_config(
    payload: &mut Value,
    request: &ModelRequestConfig,
    reasoning_effort_override: Option<&str>,
) {
    if let Some(temperature) = request.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        payload["top_p"] = json!(top_p);
    }
    if let Some(top_k) = request.top_k {
        payload["top_k"] = json!(top_k);
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        payload["max_output_tokens"] = json!(max_output_tokens);
    }
    if let Some(frequency_penalty) = request.frequency_penalty {
        payload["frequency_penalty"] = json!(frequency_penalty);
    }
    if let Some(presence_penalty) = request.presence_penalty {
        payload["presence_penalty"] = json!(presence_penalty);
    }
    if let Some(effort) = reasoning_effort_override
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .or_else(|| {
            request
                .reasoning_effort
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        })
    {
        payload["reasoning"] = json!({ "effort": effort });
    }

    for (key, value) in &request.extra_body {
        if !key.trim().is_empty() {
            payload[key] = value.clone();
        }
    }
}

fn normalize_input_items_for_responses(items: &[Value]) -> Vec<Value> {
    fn is_legacy_output_item_type(content_type: &str) -> bool {
        matches!(
            content_type,
            "function_call"
                | "function_call_output"
                | "reasoning"
                | "custom_tool_call"
                | "custom_tool_call_output"
                | "message"
        )
    }

    fn normalize_content_type(content: &mut Value, role: Option<&str>) {
        if let Some(obj) = content.as_object_mut() {
            if let Some(content_type) = obj.get("type").and_then(Value::as_str) {
                if content_type == "text" {
                    let mapped = if role == Some("assistant") {
                        "output_text"
                    } else {
                        "input_text"
                    };
                    obj.insert("type".to_string(), Value::String(mapped.to_string()));
                }
            }
        }
    }

    let mut normalized = Vec::with_capacity(items.len());
    for item in items {
        let mut cloned = item.clone();
        let role = cloned
            .get("role")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned);

        let mut extracted_output_items = Vec::new();

        if let Some(content) = cloned.get_mut("content").and_then(Value::as_array_mut) {
            let original_parts = std::mem::take(content);
            for mut part in original_parts {
                let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");

                // Backward-compat: older runtime versions wrapped output items as assistant message
                // content. Responses input expects these as top-level items.
                if role.as_deref() == Some("assistant") && is_legacy_output_item_type(part_type) {
                    extracted_output_items.push(part);
                    continue;
                }

                normalize_content_type(&mut part, role.as_deref());
                content.push(part);
            }

            if content.is_empty() {
                if let Some(obj) = cloned.as_object_mut() {
                    obj.remove("content");
                }
            }
        }

        let has_role = cloned.get("role").and_then(Value::as_str).is_some();
        let has_content = cloned
            .get("content")
            .and_then(Value::as_array)
            .map(|arr| !arr.is_empty())
            .unwrap_or(false);

        if !has_role || has_content {
            normalized.push(cloned);
        }

        normalized.extend(extracted_output_items);
    }

    normalized
}

#[cfg(test)]
mod normalization_tests {
    use super::normalize_input_items_for_responses;
    use serde_json::json;

    #[test]
    fn flattens_legacy_assistant_content_wrapped_function_call() {
        let input = vec![json!({
            "role": "assistant",
            "content": [
                {
                    "type": "function_call",
                    "id": "fc_1",
                    "call_id": "call_1",
                    "name": "tool_a",
                    "arguments": "{}"
                }
            ]
        })];

        let normalized = normalize_input_items_for_responses(&input);
        assert_eq!(normalized.len(), 1);
        assert_eq!(
            normalized[0].get("type").and_then(|v| v.as_str()),
            Some("function_call")
        );
        assert_eq!(
            normalized[0].get("call_id").and_then(|v| v.as_str()),
            Some("call_1")
        );
    }
}
