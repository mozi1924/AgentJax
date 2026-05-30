use crate::config::{ModelRequestConfig, ResolvedModelConfig};
use crate::providers::types::ResponseStreamRequest;
use serde_json::{Map, Value, json};
use std::collections::HashMap;

/// Convert the provider-neutral runtime request into Gemini GenerateContent payload.
pub(super) fn build_gemini_payload(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
) -> Result<Value, String> {
    let mut payload = json!({
        "contents": build_gemini_contents(req)?,
    });

    let instructions = req
        .instructions_override
        .as_deref()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(&resolved.system_prompt);
    if !instructions.trim().is_empty() {
        payload["systemInstruction"] = json!({
            "parts": [{ "text": instructions }]
        });
    }

    if let Some(tools) = req.tools.as_ref().filter(|tools| !tools.is_empty()) {
        payload["tools"] = json!([{
            "functionDeclarations": tools
        }]);
    }

    let generation_config = build_generation_config(&resolved.request, req);
    if !generation_config.is_empty() {
        payload["generationConfig"] = Value::Object(generation_config);
    }

    apply_extra_body(&mut payload, &resolved.request);
    Ok(payload)
}

fn build_generation_config(
    request: &ModelRequestConfig,
    req: &ResponseStreamRequest,
) -> Map<String, Value> {
    let mut config = Map::new();
    if let Some(temperature) = request.temperature {
        config.insert("temperature".to_string(), json!(temperature));
    }
    if let Some(top_p) = request.top_p {
        config.insert("topP".to_string(), json!(top_p));
    }
    if let Some(top_k) = request.top_k {
        config.insert("topK".to_string(), json!(top_k));
    }
    if let Some(max_output_tokens) = request.max_output_tokens {
        config.insert("maxOutputTokens".to_string(), json!(max_output_tokens));
    }
    if let Some(text) = &req.text {
        if let Some(format_type) = text
            .get("format")
            .and_then(|format| format.get("type"))
            .and_then(Value::as_str)
        {
            if format_type == "json_object" {
                config.insert(
                    "responseMimeType".to_string(),
                    Value::String("application/json".to_string()),
                );
            }
        }
    }
    config
}

fn apply_extra_body(payload: &mut Value, request: &ModelRequestConfig) {
    for (key, value) in &request.extra_body {
        let normalized = key.trim().to_ascii_lowercase();
        if normalized.is_empty() {
            continue;
        }
        if matches!(
            normalized.as_str(),
            "contents" | "systeminstruction" | "tools" | "generationconfig"
        ) {
            continue;
        }
        payload[key] = value.clone();
    }
}

fn build_gemini_contents(req: &ResponseStreamRequest) -> Result<Vec<Value>, String> {
    let mut contents = Vec::new();
    let mut call_names_by_id = HashMap::new();

    for item in &req.input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "function_call" | "custom_tool_call" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let name = item.get("name").and_then(Value::as_str).unwrap_or("");
                if !call_id.is_empty() && !name.is_empty() {
                    call_names_by_id.insert(call_id.to_string(), name.to_string());
                    contents.push(json!({
                        "role": "model",
                        "parts": [{
                            "functionCall": {
                                "name": name,
                                "args": parse_arguments_object(item.get("arguments"))
                            }
                        }]
                    }));
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let Some(name) = call_names_by_id.get(call_id) else {
                    continue;
                };
                contents.push(json!({
                    "role": "user",
                    "parts": [{
                        "functionResponse": {
                            "name": name,
                            "response": tool_output_response(item.get("output"))
                        }
                    }]
                }));
            }
            "reasoning" => {
                if let Some(text) = extract_reasoning_summary(item) {
                    contents.push(json!({
                        "role": "model",
                        "parts": [{ "text": text }]
                    }));
                }
            }
            _ => {
                if let Some(content) = gemini_content_from_message(item) {
                    contents.push(content);
                }
            }
        }
    }

    Ok(contents)
}

fn gemini_content_from_message(item: &Value) -> Option<Value> {
    let raw_role = item.get("role").and_then(Value::as_str)?.trim();
    let role = match raw_role {
        "assistant" => "model",
        "user" => "user",
        "system" | "developer" => "user",
        _ => "user",
    };
    let parts = gemini_parts_from_item(item)?;
    Some(json!({
        "role": role,
        "parts": parts
    }))
}

fn gemini_parts_from_item(item: &Value) -> Option<Vec<Value>> {
    let content = item.get("content")?;
    if let Some(text) = content.as_str() {
        return (!text.trim().is_empty()).then(|| vec![json!({ "text": text })]);
    }

    let parts = content.as_array()?;
    let mut gemini_parts = Vec::new();
    for part in parts {
        if let Some(text) = part
            .get("text")
            .or_else(|| part.get("input_text"))
            .and_then(Value::as_str)
        {
            if !text.is_empty() {
                gemini_parts.push(json!({ "text": text }));
            }
            continue;
        }
        if let Some(image_part) = gemini_image_part(part) {
            gemini_parts.push(image_part);
        }
    }

    (!gemini_parts.is_empty()).then_some(gemini_parts)
}

fn gemini_image_part(part: &Value) -> Option<Value> {
    let url = part
        .get("image_url")
        .and_then(|value| {
            value
                .as_str()
                .or_else(|| value.get("url").and_then(Value::as_str))
        })
        .or_else(|| part.get("image").and_then(Value::as_str))?;

    if let Some(data) = url.strip_prefix("data:") {
        let (mime_type, encoded) = data.split_once(";base64,")?;
        return Some(json!({
            "inlineData": {
                "mimeType": mime_type,
                "data": encoded
            }
        }));
    }

    Some(json!({
        "fileData": {
            "mimeType": part
                .get("mime_type")
                .and_then(Value::as_str)
                .unwrap_or("image/jpeg"),
            "fileUri": url
        }
    }))
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

fn tool_output_response(output: Option<&Value>) -> Value {
    let text = output.and_then(Value::as_str).unwrap_or("");
    if let Ok(parsed) = serde_json::from_str::<Value>(text) {
        if parsed.is_object() {
            return parsed;
        }
    }
    json!({ "result": text })
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
