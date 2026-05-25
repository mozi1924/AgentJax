use serde_json::{json, Value};

use crate::config::{ModelRequestConfig, ResolvedModelConfig};
use crate::providers::types::ResponseStreamRequest;

pub(crate) fn build_streaming_request_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    previous_response_id: Option<&str>,
    store: bool,
) -> Value {
    let previous_response_id = previous_response_id
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(ToOwned::to_owned);

    let input_items = normalize_input_items_for_responses(&req.input_items);

    let mut payload = json!({
      "model": resolved.model_id,
      "instructions": req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.provider.system_prompt),
      "input": input_items,
      "store": store,
      "stream": true
    });

    apply_model_request_config(
        &mut payload,
        &resolved.request,
        req.reasoning_effort.as_deref(),
    );

    if let Some(previous_id) = previous_response_id {
        payload["previous_response_id"] = Value::String(previous_id);
    }

    if let Some(tools) = &req.tools {
        if !tools.is_empty() {
            payload["tools"] = json!(tools);
        }
    }
    if let Some(tool_choice) = &req.tool_choice {
        payload["tool_choice"] = tool_choice.clone();
    }

    payload
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
        if let Some(content) = cloned.get_mut("content").and_then(Value::as_array_mut) {
            for part in content {
                normalize_content_type(part, role.as_deref());
            }
        }
        normalized.push(cloned);
    }

    normalized
}
