use crate::config::{ModelRequestConfig, ResolvedModelConfig};
use crate::providers::types::ResponseStreamRequest;
use serde_json::{Value, json};

#[derive(Debug, Clone, Default)]
struct AnthropicPayloadMessages {
    system_sections: Vec<String>,
    messages: Vec<Value>,
}

/// Convert AgentJax's provider-neutral request shape into Anthropic Messages API payload.
pub(super) fn build_anthropic_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Value, String> {
    let AnthropicPayloadMessages {
        mut system_sections,
        messages,
    } = build_anthropic_messages(req)?;
    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt);
    if !instructions.trim().is_empty() {
        system_sections.insert(0, instructions.trim().to_string());
    }

    let mut payload = json!({
        "model": resolved.model_id,
        "max_tokens": resolved.request.max_output_tokens.unwrap_or(4096),
        "messages": messages,
        "stream": true
    });
    if !system_sections.is_empty() {
        payload["system"] = Value::String(system_sections.join("\n\n"));
    }
    apply_generation_config(&mut payload, &resolved.request);
    if let Some(tools) = req.tools.as_ref().filter(|tools| !tools.is_empty()) {
        payload["tools"] = json!(tools);
        if let Some(tool_choice) = normalize_tool_choice(req.tool_choice.as_ref()) {
            payload["tool_choice"] = tool_choice;
        }
    }
    apply_extra_body(&mut payload, &resolved.request);
    Ok(payload)
}

fn apply_generation_config(payload: &mut Value, request: &ModelRequestConfig) {
    if let Some(temperature) = request.temperature {
        payload["temperature"] = json!(temperature);
    }
    if let Some(top_p) = request.top_p {
        payload["top_p"] = json!(top_p);
    }
    if let Some(top_k) = request.top_k {
        payload["top_k"] = json!(top_k);
    }
}

fn normalize_tool_choice(tool_choice: Option<&Value>) -> Option<Value> {
    match tool_choice? {
        // Runtime requests use the OpenAI-compatible string form. Anthropic
        // expects a typed object, so normalize common choices at the adapter
        // boundary before the payload leaves AgentJax.
        Value::String(choice) => {
            let choice = choice.trim().to_ascii_lowercase();
            if choice.is_empty() {
                None
            } else if choice == "required" {
                Some(json!({ "type": "any" }))
            } else {
                Some(json!({ "type": choice }))
            }
        }
        Value::Object(obj) if !obj.is_empty() => Some(Value::Object(obj.clone())),
        _ => None,
    }
}

fn apply_extra_body(payload: &mut Value, request: &ModelRequestConfig) {
    for (key, value) in &request.extra_body {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if matches!(
            normalized.as_str(),
            "model" | "messages" | "system" | "stream" | "tools"
        ) {
            continue;
        }
        payload[key] = value.clone();
    }
}

fn build_anthropic_messages(
    req: &ResponseStreamRequest,
) -> Result<AnthropicPayloadMessages, String> {
    let mut out = AnthropicPayloadMessages::default();

    for item in &req.input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "function_call" | "custom_tool_call" => {
                if let Some(message) = anthropic_tool_use_message(item)? {
                    out.messages.push(message);
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                if let Some(message) = anthropic_tool_result_message(item) {
                    out.messages.push(message);
                }
            }
            "reasoning" => {
                if let Some(text) = extract_reasoning_summary(item) {
                    out.messages.push(json!({
                        "role": "assistant",
                        "content": [{ "type": "text", "text": text }]
                    }));
                }
            }
            _ => {
                if let Some((system_text, message)) = anthropic_message_from_item(item)? {
                    if !system_text.trim().is_empty() {
                        out.system_sections.push(system_text);
                    }
                    if let Some(message) = message {
                        out.messages.push(message);
                    }
                }
            }
        }
    }

    Ok(out)
}

fn anthropic_message_from_item(item: &Value) -> Result<Option<(String, Option<Value>)>, String> {
    let raw_role = item
        .get("role")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim();
    if raw_role.is_empty() {
        return Ok(None);
    }
    let content = anthropic_content_parts(item)?;
    if content.is_empty() {
        return Ok(None);
    }
    if matches!(raw_role, "system" | "developer") {
        return Ok(Some((text_from_anthropic_parts(&content), None)));
    }
    let role = if raw_role == "assistant" {
        "assistant"
    } else {
        "user"
    };
    Ok(Some((
        String::new(),
        Some(json!({
            "role": role,
            "content": content
        })),
    )))
}

fn anthropic_tool_use_message(item: &Value) -> Result<Option<Value>, String> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    let name = item.get("name").and_then(Value::as_str).unwrap_or("");
    if call_id.trim().is_empty() || name.trim().is_empty() {
        return Ok(None);
    }
    Ok(Some(json!({
        "role": "assistant",
        "content": [{
            "type": "tool_use",
            "id": call_id,
            "name": name,
            "input": parse_arguments_object(item.get("arguments"))
        }]
    })))
}

fn anthropic_tool_result_message(item: &Value) -> Option<Value> {
    let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
    if call_id.trim().is_empty() {
        return None;
    }
    Some(json!({
        "role": "user",
        "content": [{
            "type": "tool_result",
            "tool_use_id": call_id,
            "content": item.get("output").and_then(Value::as_str).unwrap_or("")
        }]
    }))
}

fn anthropic_content_parts(item: &Value) -> Result<Vec<Value>, String> {
    let Some(content) = item.get("content") else {
        return Ok(Vec::new());
    };
    if let Some(text) = content.as_str() {
        return Ok((!text.trim().is_empty())
            .then(|| vec![json!({ "type": "text", "text": text })])
            .unwrap_or_default());
    }

    let Some(parts) = content.as_array() else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for part in parts {
        if let Some(text) = part
            .get("text")
            .or_else(|| part.get("input_text"))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                out.push(json!({ "type": "text", "text": text }));
            }
            continue;
        }
        if let Some(image) = anthropic_image_part(part) {
            out.push(image);
        }
    }
    Ok(out)
}

fn anthropic_image_part(part: &Value) -> Option<Value> {
    let url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })
        .or_else(|| part.get("image").and_then(Value::as_str))?;

    if let Some(data) = url.strip_prefix("data:") {
        let (media_type, encoded) = data.split_once(";base64,")?;
        return Some(json!({
            "type": "image",
            "source": {
                "type": "base64",
                "media_type": media_type,
                "data": encoded
            }
        }));
    }

    Some(json!({
        "type": "image",
        "source": {
            "type": "url",
            "url": url
        }
    }))
}

fn text_from_anthropic_parts(parts: &[Value]) -> String {
    parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn parse_arguments_object(arguments: Option<&Value>) -> Value {
    let Some(arguments) = arguments else {
        return json!({});
    };
    if arguments.is_object() {
        return arguments.clone();
    }
    if let Some(arguments) = arguments.as_str() {
        return serde_json::from_str(arguments).unwrap_or_else(|_| json!({}));
    }
    json!({})
}

fn extract_reasoning_summary(item: &Value) -> Option<String> {
    let summary = item.get("summary")?;
    if let Some(text) = summary.as_str() {
        return (!text.trim().is_empty()).then(|| text.trim().to_string());
    }
    let parts = summary.as_array()?;
    let text = parts
        .iter()
        .filter_map(|part| part.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("");
    (!text.trim().is_empty()).then(|| text.trim().to_string())
}
