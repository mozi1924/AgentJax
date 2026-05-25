pub mod models;
pub mod stream;

use super::types::ProviderPendingToolCall;
use serde_json::{json, Value};

pub fn normalize_reasoning_levels(levels: &[String]) -> Vec<String> {
    let mut normalized = Vec::new();

    for level in levels {
        let level = level.trim().to_lowercase();
        if !matches!(
            level.as_str(),
            "none" | "minimal" | "low" | "medium" | "high" | "xhigh"
        ) {
            continue;
        }

        if !normalized.iter().any(|existing| existing == &level) {
            normalized.push(level);
        }
    }

    normalized
}

pub fn infer_reasoning_levels_from_model_id(model_id: &str) -> Vec<String> {
    let model = model_id.trim().to_lowercase();
    if model.is_empty() {
        return Vec::new();
    }

    if model.starts_with("gpt-5-pro") {
        return vec!["high".to_string()];
    }

    if model.starts_with("gpt-5.2-pro") {
        return vec![
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    if model.starts_with("gpt-5.2-codex") {
        return vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    if model.starts_with("gpt-5.2") {
        return vec![
            "none".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    if model.starts_with("gpt-5.1") {
        return vec![
            "none".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ];
    }

    if model.starts_with("gpt-5") {
        return vec![
            "minimal".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ];
    }

    if model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4") {
        return vec!["low".to_string(), "medium".to_string(), "high".to_string()];
    }

    Vec::new()
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
        if let Ok(parsed) = serde_json::from_str::<Value>(trimmed) {
            if parsed.is_object() {
                return parsed;
            }
        }
        return json!({});
    }

    json!({})
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
    let mut items: Vec<Value> = output_items
        .iter()
        .filter(|item| {
            matches!(
                item.get("type").and_then(Value::as_str),
                Some("reasoning") | Some("function_call") | Some("custom_tool_call")
            )
        })
        .cloned()
        .collect();
    items.append(&mut tool_results_items);
    items
}

#[cfg(test)]
mod tests {
    use super::{extract_pending_tool_calls_from_output, infer_reasoning_levels_from_model_id};
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
    fn infers_reasoning_levels_for_gpt_5_2_codex() {
        let levels = infer_reasoning_levels_from_model_id("gpt-5.2-codex");
        assert_eq!(levels, vec!["low", "medium", "high", "xhigh"]);
    }

    #[test]
    fn infers_reasoning_levels_for_gpt_5_1() {
        let levels = infer_reasoning_levels_from_model_id("gpt-5.1");
        assert_eq!(levels, vec!["none", "low", "medium", "high"]);
    }

    #[test]
    fn infers_reasoning_levels_for_gpt_5_pro() {
        let levels = infer_reasoning_levels_from_model_id("gpt-5-pro");
        assert_eq!(levels, vec!["high"]);
    }
}
