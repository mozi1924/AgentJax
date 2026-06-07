use crate::provider_api::types::ProviderPendingToolCall;
use serde_json::{Value, json};
use uuid::Uuid;

/// Generates AgentJax-local IDs for upstream protocols that do not expose all
/// Responses-style identifiers. The generated IDs are stable within one adapter
/// response hop and intentionally carry the provider prefix to simplify log
/// inspection when debugging multi-provider conversations.
#[derive(Debug, Clone)]
pub struct ProviderIdFactory {
    response_id: String,
}

impl ProviderIdFactory {
    pub fn new(provider_kind: &str) -> Self {
        let provider_kind = sanitize_id_segment(provider_kind);
        let response_id = format!("resp_{}_{}", provider_kind, Uuid::new_v4().simple());

        Self { response_id }
    }

    pub fn response_id(&self) -> &str {
        &self.response_id
    }
}

fn sanitize_id_segment(value: &str) -> String {
    let mut normalized = String::new();

    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
        } else if !normalized.ends_with('_') {
            normalized.push('_');
        }
    }

    let normalized = normalized.trim_matches('_').to_string();
    if normalized.is_empty() {
        "provider".to_string()
    } else {
        normalized
    }
}

pub fn extract_pending_tool_calls_from_output(
    output_items: &[Value],
) -> Vec<ProviderPendingToolCall> {
    let mut pending_tools: Vec<ProviderPendingToolCall> = Vec::new();

    for item in output_items {
        if item.get("type").and_then(Value::as_str) != Some("function_call") {
            continue;
        }

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
        let arguments = parse_tool_arguments_value(item.get("arguments"));

        if call_id.is_empty() || name.is_empty() {
            continue;
        }

        let has_output = output_items.iter().any(|other| {
            other.get("type").and_then(Value::as_str) == Some("function_call_output")
                && other.get("call_id").and_then(Value::as_str) == Some(call_id.as_str())
        });

        if has_output {
            continue;
        }

        if let Some(existing) = pending_tools
            .iter_mut()
            .find(|call| call.call_id.as_str() == call_id.as_str())
        {
            existing.name = name;
            existing.arguments = arguments;
            continue;
        }

        pending_tools.push(ProviderPendingToolCall {
            call_id,
            name,
            arguments,
        });
    }

    pending_tools
}

pub fn build_tool_result_input_item(call_id: &str, output: &str) -> Value {
    json!({
        "type": "function_call_output",
        "call_id": call_id,
        "output": output,
    })
}

pub fn build_user_input_item(text: &str) -> Value {
    json!({
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": text
        }]
    })
}

pub fn compose_tool_continuation_input(
    output_items: &[Value],
    mut tool_results_items: Vec<Value>,
) -> Vec<Value> {
    // Preserve the full logical order of the model's response:
    //   reasoning → assistant text → function_calls → tool_results
    // Previously, assistant-text ("message" type) items were dropped,
    // losing any output text the model produced between reasoning and
    // tool calls. This matters for think models where the model may
    // emit partial text (e.g. "Let me search for...") before invoking
    // tools — that text provides valuable context for the next hop.
    let mut items: Vec<Value> = output_items
        .iter()
        .filter(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("reasoning") | Some("function_call") | Some("custom_tool_call") | Some("message")
            )
        })
        .cloned()
        .collect();
    items.append(&mut tool_results_items);
    items
}

fn parse_tool_arguments_value(raw: Option<&Value>) -> Value {
    let Some(raw) = raw else {
        return json!({});
    };

    if raw.is_object() {
        return raw.clone();
    }

    if let Some(arguments_str) = raw.as_str() {
        let trimmed = arguments_str.trim();
        if trimmed.is_empty() {
            return json!({});
        }
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed)
            && parsed.is_object()
        {
            return parsed;
        }
        return json!({});
    }

    json!({})
}

#[cfg(test)]
mod tests {
    use super::{
        ProviderIdFactory, build_tool_result_input_item, build_user_input_item,
        compose_tool_continuation_input, extract_pending_tool_calls_from_output,
    };
    use serde_json::json;

    #[test]
    fn parses_stringified_function_call_arguments_into_object() {
        let output_items = vec![json!({
            "type": "function_call",
            "call_id": "call_a",
            "name": "tool_a",
            "arguments": "{\"x\":1}"
        })];

        let pending = extract_pending_tool_calls_from_output(&output_items);
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].arguments.get("x").and_then(|v| v.as_i64()),
            Some(1)
        );
    }

    #[test]
    fn falls_back_to_empty_object_when_arguments_are_invalid() {
        let output_items = vec![json!({
            "type": "function_call",
            "call_id": "call_a",
            "name": "tool_a",
            "arguments": "not-json"
        })];

        let pending = extract_pending_tool_calls_from_output(&output_items);
        assert_eq!(pending.len(), 1);
        assert!(pending[0].arguments.is_object());
    }

    #[test]
    fn builds_user_and_tool_items_and_continuation() {
        let user_input = build_user_input_item("hello");
        assert_eq!(
            user_input.get("role").and_then(|v| v.as_str()),
            Some("user")
        );

        let tool_output = build_tool_result_input_item("call_1", "ok");
        assert_eq!(
            tool_output.get("type").and_then(|v| v.as_str()),
            Some("function_call_output")
        );

        let continuation = compose_tool_continuation_input(
            &[
                json!({"type":"reasoning","id":"r1"}),
                json!({"type":"function_call","call_id":"call_1"}),
            ],
            vec![tool_output],
        );

        assert_eq!(continuation.len(), 3);
    }

    #[test]
    fn synthetic_response_id_is_prefixed() {
        let factory = ProviderIdFactory::new("gemini-native");
        assert!(factory.response_id().starts_with("resp_gemini_native_"));
    }
}
