use serde_json::Value;
use std::collections::HashSet;

/// Remove orphaned tool call or output entries before the request is sent.
///
/// This keeps the final payload internally consistent when only one side of a
/// persisted tool pair survived recovery or editing.
pub(super) fn sanitize_tool_call_pairs(items: Vec<Value>) -> Vec<Value> {
    let mut function_call_ids = HashSet::new();
    let mut function_call_output_ids = HashSet::new();

    for item in &items {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    function_call_ids.insert(call_id.to_string());
                }
            }
            Some("function_call_output") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    function_call_output_ids.insert(call_id.to_string());
                }
            }
            _ => {}
        }
    }

    items
        .into_iter()
        .filter(|item| match item.get("type").and_then(Value::as_str) {
            Some("function_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| function_call_output_ids.contains(call_id))
                .unwrap_or(false),
            Some("function_call_output") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| function_call_ids.contains(call_id))
                .unwrap_or(false),
            _ => true,
        })
        .collect()
}
