use crate::message_phase::AssistantPhase;
use serde_json::Value;

pub(crate) fn extract_output_items(root: &Value) -> Vec<Value> {
    root.get("output")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

struct AssistantMessageChunk {
    text: String,
    phase: Option<AssistantPhase>,
}

fn extract_assistant_messages(root: &Value) -> Vec<AssistantMessageChunk> {
    let Some(output) = root.get("output").and_then(Value::as_array) else {
        return Vec::new();
    };

    output
        .iter()
        .filter(|item| {
            item.get("type").and_then(Value::as_str) == Some("message")
                && item.get("role").and_then(Value::as_str) == Some("assistant")
        })
        .filter_map(|item| {
            let text = item
                .get("content")
                .and_then(Value::as_array)
                .map(|content| {
                    content
                        .iter()
                        .filter_map(|part| part.get("text").and_then(Value::as_str))
                        .collect::<Vec<_>>()
                        .join("")
                })
                .unwrap_or_default();
            if text.trim().is_empty() {
                return None;
            }
            Some(AssistantMessageChunk {
                text,
                phase: item
                    .get("phase")
                    .and_then(Value::as_str)
                    .and_then(AssistantPhase::from_api_value),
            })
        })
        .collect()
}

fn extract_final_output_text(root: &Value) -> String {
    let assistant_messages = extract_assistant_messages(root);
    if let Some(final_text) = assistant_messages
        .iter()
        .rev()
        .find(|message| message.phase != Some(AssistantPhase::Commentary))
        .map(|message| message.text.clone())
    {
        return final_text;
    }

    assistant_messages
        .last()
        .map(|message| message.text.clone())
        .unwrap_or_default()
}

pub(crate) fn extract_output_text(root: &Value) -> String {
    let final_output_text = extract_final_output_text(root);
    if !final_output_text.is_empty() {
        return final_output_text;
    }

    if let Some(choices) = root.get("choices").and_then(Value::as_array) {
        if let Some(first) = choices.first() {
            if let Some(message) = first.get("message") {
                if let Some(content) = message.get("content").and_then(Value::as_str) {
                    return content.to_string();
                }
                if let Some(content_items) = message.get("content").and_then(Value::as_array) {
                    let joined = content_items
                        .iter()
                        .filter_map(value_to_text)
                        .collect::<Vec<_>>()
                        .join("");
                    if !joined.is_empty() {
                        return joined;
                    }
                }
            }
            if let Some(text) = first.get("text").and_then(Value::as_str) {
                return text.to_string();
            }
        }
    }

    if let Some(text) = root.get("text").and_then(Value::as_str) {
        return text.to_string();
    }

    String::new()
}

fn value_to_text(value: &Value) -> Option<String> {
    if let Some(s) = value.as_str() {
        return Some(s.to_string());
    }
    value
        .get("text")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
}
