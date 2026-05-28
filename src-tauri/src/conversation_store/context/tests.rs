use super::sanitizer::sanitize_tool_call_pairs;
use super::truncation::truncate_context_items_preserving_tool_pairs;
use serde_json::json;

#[test]
fn sanitize_tool_call_pairs_removes_orphans() {
    let items = vec![
        json!({"type":"function_call","call_id":"keep","name":"x","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"keep","output":"{}"}),
        json!({"type":"function_call","call_id":"drop_call","name":"x","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"drop_output","output":"{}"}),
    ];

    let sanitized = sanitize_tool_call_pairs(items);
    let call_ids: Vec<String> = sanitized
        .iter()
        .filter_map(|item| item.get("call_id").and_then(|v| v.as_str()))
        .map(|call_id| call_id.to_string())
        .collect();

    assert_eq!(call_ids, vec!["keep", "keep"]);
}

#[test]
fn truncate_context_items_preserves_tool_pair_boundaries() {
    let items = vec![
        json!({"type":"user","id":"u1"}),
        json!({"type":"function_call","call_id":"keep","name":"x","arguments":"{}"}),
        json!({"type":"function_call_output","call_id":"keep","output":"{}"}),
    ];

    let truncated = truncate_context_items_preserving_tool_pairs(items, 1);
    assert_eq!(truncated.len(), 2);
    assert!(truncated
        .iter()
        .any(|item| item.get("call_id").and_then(|v| v.as_str()) == Some("keep")));
}
