use crate::provider_api::types::ProviderPendingToolCall;
use serde_json::{Value, json};

pub(super) fn describe_item_shape(item: &Value) -> String {
    if let Some(kind) = item.get("type").and_then(Value::as_str) {
        return format!("type:{kind}");
    }
    if let Some(role) = item.get("role").and_then(Value::as_str) {
        return format!("role:{role}");
    }
    "unknown".to_string()
}

pub(super) fn push_or_update_pending_tool_call(
    calls: &mut Vec<ProviderPendingToolCall>,
    call_id: String,
    name: String,
    arguments: Value,
) {
    if let Some(existing) = calls.iter_mut().find(|call| call.call_id == call_id) {
        existing.name = name;
        existing.arguments = arguments;
        return;
    }

    calls.push(ProviderPendingToolCall {
        call_id,
        name,
        arguments,
    });
}

pub(super) fn parse_tool_arguments(arguments: &str, fallback_delta: Option<&str>) -> Value {
    let parse_json_object = |raw: &str| -> Option<Value> {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            return None;
        }

        let parsed = serde_json::from_str::<Value>(trimmed).ok()?;
        if parsed.is_object() {
            Some(parsed)
        } else {
            None
        }
    };

    parse_json_object(arguments)
        .or_else(|| fallback_delta.and_then(parse_json_object))
        .unwrap_or_else(|| json!({}))
}

pub(super) fn is_valid_pending_tool_call(call: &ProviderPendingToolCall) -> bool {
    !call.call_id.trim().is_empty() && !call.name.trim().is_empty()
}

#[cfg(test)]
pub(super) fn extract_active_tool_names(
    tools_schemas: &[Value],
) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for schema in tools_schemas {
        if let Some(name) = schema.get("name").and_then(Value::as_str) {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                names.insert(trimmed.to_string());
            }
            continue;
        }

        if let Some(name) = schema
            .get("function")
            .and_then(|v| v.get("name"))
            .and_then(Value::as_str)
        {
            let trimmed = name.trim();
            if !trimmed.is_empty() {
                names.insert(trimmed.to_string());
            }
        }
    }
    names
}
