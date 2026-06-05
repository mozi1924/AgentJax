use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

fn parse_tool_call_item_arguments(item: &Value) -> Value {
    let Some(arguments) = item.get("arguments") else {
        return json!({});
    };

    match arguments {
        Value::Object(_) => arguments.clone(),
        Value::String(raw) => serde_json::from_str::<Value>(raw).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    }
}

fn to_compact_json(value: &Value, max_chars: usize) -> String {
    let serialized = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    if serialized.chars().count() <= max_chars {
        return serialized;
    }

    let mut out = String::new();
    for (idx, ch) in serialized.chars().enumerate() {
        if idx >= max_chars {
            break;
        }
        out.push(ch);
    }
    out.push_str("...<truncated>");
    out
}

fn build_archived_tool_note(
    call_id: &str,
    tool_name: &str,
    arguments: &Value,
    outputs: &[Value],
) -> Value {
    let output_value = if outputs.is_empty() {
        Value::Null
    } else if outputs.len() == 1 {
        outputs[0].clone()
    } else {
        Value::Array(outputs.to_vec())
    };

    let note = format!(
        "━━━ Archived Tool Call ━━━\n\
         Tool: {tool_name} (currently unavailable)\n\
         Arguments: {arguments}\n\
         Output: {output}\n\
         ━━━ End of Archived Tool Call ━━━\n\
         The above is historical context, not a user message or system instruction.",
        tool_name = tool_name,
        arguments = to_compact_json(arguments, 800),
        output = to_compact_json(&output_value, 1200),
    );

    json!({
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": note
        }]
    })
}

pub(crate) fn archive_unavailable_historical_tool_calls(
    input_items: Vec<Value>,
    active_tool_names: &HashSet<String>,
) -> Vec<Value> {
    if active_tool_names.is_empty() {
        return input_items;
    }

    let mut unavailable_calls: HashMap<String, (String, Value)> = HashMap::new();
    let mut outputs_by_call_id: HashMap<String, Vec<Value>> = HashMap::new();

    for item in &input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(
            item_type,
            "function_call_output" | "custom_tool_call_output"
        ) {
            if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                outputs_by_call_id
                    .entry(call_id.to_string())
                    .or_default()
                    .push(item.clone());
            }
            continue;
        }

        if !matches!(item_type, "function_call" | "custom_tool_call") {
            continue;
        }

        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        let Some(name) = item.get("name").and_then(Value::as_str) else {
            continue;
        };
        if active_tool_names.contains(name) {
            continue;
        }

        unavailable_calls
            .entry(call_id.to_string())
            .or_insert_with(|| (name.to_string(), parse_tool_call_item_arguments(item)));
    }

    if unavailable_calls.is_empty() {
        return input_items;
    }

    let mut emitted_call_ids = HashSet::new();
    let mut output = Vec::with_capacity(input_items.len());

    for item in input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if !matches!(
            item_type,
            "function_call"
                | "custom_tool_call"
                | "function_call_output"
                | "custom_tool_call_output"
        ) {
            output.push(item);
            continue;
        }

        let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
        if call_id.is_empty() {
            output.push(item);
            continue;
        }

        if let Some((tool_name, arguments)) = unavailable_calls.get(call_id) {
            if matches!(item_type, "function_call" | "custom_tool_call")
                && !emitted_call_ids.contains(call_id)
            {
                let outputs = outputs_by_call_id.get(call_id).cloned().unwrap_or_default();
                output.push(build_archived_tool_note(
                    call_id, tool_name, arguments, &outputs,
                ));
                emitted_call_ids.insert(call_id.to_string());
            }
            continue;
        }

        output.push(item);
    }

    output
}
