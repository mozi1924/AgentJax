use super::multimodal::{estimate_image_tokens, is_image_part};
use super::types::{MessageContentEstimate, TokenCountFunctionCall, TokenCountMessage};
use serde_json::{Value, json};
use std::collections::HashMap;

pub(super) fn build_chat_completion_messages(
    items: &[Value],
) -> Result<Vec<TokenCountMessage>, String> {
    let mut messages = Vec::new();
    let mut tool_names_by_call_id: HashMap<String, String> = HashMap::new();

    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "message" => {
                if let Some(message) = build_message_from_item(item) {
                    messages.push(message);
                }
            }
            "function_call" | "custom_tool_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                if !call_id.is_empty() && !name.is_empty() {
                    tool_names_by_call_id.insert(call_id, name.clone());
                }

                if !name.is_empty() {
                    messages.push(TokenCountMessage {
                        role: "assistant".to_string(),
                        content: None,
                        name: None,
                        function_call: Some(TokenCountFunctionCall { name, arguments }),
                        multimodal_tokens: 0,
                    });
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let output = item
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if output.trim().is_empty() {
                    continue;
                }

                let name = tool_names_by_call_id.get(call_id).cloned();
                messages.push(TokenCountMessage {
                    role: "tool".to_string(),
                    content: Some(output),
                    name,
                    function_call: None,
                    multimodal_tokens: 0,
                });
            }
            "reasoning" => {
                if let Some(summary) = extract_reasoning_summary(item) {
                    messages.push(TokenCountMessage {
                        role: "assistant".to_string(),
                        content: Some(summary),
                        name: None,
                        function_call: None,
                        multimodal_tokens: 0,
                    });
                }
            }
            _ => {
                if let Some(message) = build_message_from_item(item) {
                    messages.push(message);
                }
            }
        }
    }

    Ok(messages)
}

pub(super) fn build_system_input_item(text: &str) -> Value {
    json!({
        "role": "system",
        "content": [{
            "type": "input_text",
            "text": text,
        }]
    })
}

fn build_message_from_item(item: &Value) -> Option<TokenCountMessage> {
    let role = item.get("role").and_then(Value::as_str)?.trim();
    if role.is_empty() {
        return None;
    }

    let content = extract_message_content(item)?;
    if content.text.trim().is_empty() && content.multimodal_tokens == 0 {
        return None;
    }

    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Some(TokenCountMessage {
        role: role.to_string(),
        content: (!content.text.trim().is_empty()).then_some(content.text),
        name,
        function_call: None,
        multimodal_tokens: content.multimodal_tokens,
    })
}

fn extract_message_content(item: &Value) -> Option<MessageContentEstimate> {
    if let Some(content) = item.get("content") {
        if let Some(text) = content.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| MessageContentEstimate {
                text: trimmed.to_string(),
                multimodal_tokens: 0,
            });
        }

        if let Some(parts) = content.as_array() {
            let mut estimate = MessageContentEstimate::default();
            for part in parts {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    estimate.text.push_str(part_text);
                } else if let Some(part_text) = part.get("input_text").and_then(Value::as_str) {
                    estimate.text.push_str(part_text);
                }

                if is_image_part(part) {
                    estimate.multimodal_tokens = estimate
                        .multimodal_tokens
                        .saturating_add(estimate_image_tokens(part));
                }
            }
            let trimmed = estimate.text.trim();
            if !trimmed.is_empty() || estimate.multimodal_tokens > 0 {
                estimate.text = trimmed.to_string();
                return Some(estimate);
            }
        }
    }

    item.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| MessageContentEstimate {
            text: text.to_string(),
            multimodal_tokens: 0,
        })
}

fn extract_reasoning_summary(item: &Value) -> Option<String> {
    if let Some(summary) = item.get("summary") {
        if let Some(text) = summary.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }

        if let Some(parts) = summary.as_array() {
            let mut text = String::new();
            for part in parts {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    text.push_str(part_text);
                }
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    item.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}
