use crate::config::{ModelRequestConfig, ResolvedModelConfig};
use crate::providers::types::ResponseStreamRequest;
use serde_json::{Value, json};

/// Convert the provider-neutral runtime request into an OpenAI-compatible chat payload.
pub(super) fn build_chat_completions_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Value, String> {
    let mut payload = json!({
        "model": resolved.model_id,
        "messages": build_chat_messages(resolved, req)?,
        "stream": true,
        "stream_options": {
            "include_usage": true
        }
    });

    apply_chat_model_request_config(
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
        if let Some(format) = text.get("format") {
            payload["response_format"] = format.clone();
        }
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
        payload["metadata"] = client_metadata.clone();
    }

    Ok(payload)
}

fn apply_chat_model_request_config(
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
        payload["max_tokens"] = json!(max_output_tokens);
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
        payload["reasoning_effort"] = json!(effort);
    }

    for (key, value) in &request.extra_body {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if matches!(
            normalized.as_str(),
            "model" | "messages" | "stream" | "stream_options"
        ) {
            continue;
        }

        payload[key] = value.clone();
    }
}

fn build_chat_messages(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Vec<Value>, String> {
    let mut messages = Vec::new();
    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt);
    if !instructions.trim().is_empty() {
        messages.push(json!({
            "role": "system",
            "content": instructions
        }));
    }

    for item in &req.input_items {
        append_chat_message_from_input_item(item, &mut messages)?;
    }

    Ok(messages)
}

fn append_chat_message_from_input_item(
    item: &Value,
    messages: &mut Vec<Value>,
) -> Result<(), String> {
    let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
    match item_type {
        "function_call" | "custom_tool_call" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            let name = item.get("name").and_then(Value::as_str).unwrap_or("");
            if call_id.trim().is_empty() || name.trim().is_empty() {
                return Ok(());
            }
            let arguments = stringify_arguments(item.get("arguments"))?;
            messages.push(json!({
                "role": "assistant",
                "content": Value::Null,
                "tool_calls": [{
                    "id": call_id,
                    "type": "function",
                    "function": {
                        "name": name,
                        "arguments": arguments
                    }
                }]
            }));
        }
        "function_call_output" | "custom_tool_call_output" => {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if call_id.trim().is_empty() {
                return Ok(());
            }
            messages.push(json!({
                "role": "tool",
                "tool_call_id": call_id,
                "content": item.get("output").and_then(Value::as_str).unwrap_or("")
            }));
        }
        "reasoning" => {
            if let Some(summary) = extract_reasoning_summary(item) {
                messages.push(json!({
                    "role": "assistant",
                    "content": summary
                }));
            }
        }
        _ => {
            if let Some(message) = build_role_message(item) {
                messages.push(message);
            }
        }
    }

    Ok(())
}

fn build_role_message(item: &Value) -> Option<Value> {
    let role = item.get("role").and_then(Value::as_str)?.trim();
    if role.is_empty() {
        return None;
    }
    let content = chat_content_from_item(item)?;
    Some(json!({
        "role": role,
        "content": content
    }))
}

fn chat_content_from_item(item: &Value) -> Option<Value> {
    let content = item.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(Value::String(text.to_string()));
    }

    let parts = content.as_array()?;
    let mut text = String::new();
    let mut chat_parts = Vec::new();
    for part in parts {
        if let Some(part_text) = part
            .get("text")
            .or_else(|| part.get("input_text"))
            .and_then(Value::as_str)
        {
            text.push_str(part_text);
            continue;
        }
        if let Some(image_part) = chat_image_part(part) {
            if !text.is_empty() {
                chat_parts.push(json!({
                    "type": "text",
                    "text": std::mem::take(&mut text)
                }));
            }
            chat_parts.push(image_part);
        }
    }

    if chat_parts.is_empty() {
        return (!text.trim().is_empty()).then_some(Value::String(text));
    }
    if !text.is_empty() {
        chat_parts.push(json!({
            "type": "text",
            "text": text
        }));
    }
    Some(Value::Array(chat_parts))
}

fn chat_image_part(part: &Value) -> Option<Value> {
    let image_url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .map(|url| json!({ "url": url }))
                .or_else(|| value.as_object().map(|_| value.clone()))
        })
        .or_else(|| {
            part.get("image")
                .and_then(Value::as_str)
                .map(|url| json!({ "url": url }))
        })?;

    Some(json!({
        "type": "image_url",
        "image_url": image_url
    }))
}

fn stringify_arguments(arguments: Option<&Value>) -> Result<String, String> {
    let Some(arguments) = arguments else {
        return Ok("{}".to_string());
    };
    if let Some(arguments) = arguments.as_str() {
        return Ok(arguments.to_string());
    }
    serde_json::to_string(arguments)
        .map_err(|err| format!("Failed to serialize Chat Completions tool arguments: {err}"))
}

fn extract_reasoning_summary(item: &Value) -> Option<String> {
    if let Some(summary) = item.get("summary") {
        if let Some(text) = summary.as_str() {
            return (!text.trim().is_empty()).then(|| text.trim().to_string());
        }
        if let Some(parts) = summary.as_array() {
            let text = parts
                .iter()
                .filter_map(|part| part.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>()
                .join("");
            return (!text.trim().is_empty()).then(|| text.trim().to_string());
        }
    }
    None
}
