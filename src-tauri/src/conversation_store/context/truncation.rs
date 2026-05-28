use serde_json::Value;

/// Truncate the rebuilt context without splitting a function-call pair.
///
/// The context budget is intentionally soft here: if the last item in the
/// retained window is half of a tool pair, we widen the window just enough to
/// keep the pair intact.
pub(super) fn truncate_context_items_preserving_tool_pairs(
    items: Vec<Value>,
    max: usize,
) -> Vec<Value> {
    if items.len() <= max {
        return items;
    }

    let mut start = items.len() - max;
    while start > 0 && splits_tool_pair(&items, start) {
        start -= 1;
    }

    items.into_iter().skip(start).collect()
}

fn splits_tool_pair(items: &[Value], start: usize) -> bool {
    if start == 0 || start >= items.len() {
        return false;
    }

    let left = &items[start - 1];
    let right = &items[start];

    is_function_call(left) && is_function_call_output(right) && call_id(left) == call_id(right)
}

fn is_function_call(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("function_call")
}

fn is_function_call_output(item: &Value) -> bool {
    item.get("type").and_then(Value::as_str) == Some("function_call_output")
}

fn call_id(item: &Value) -> Option<&str> {
    item.get("call_id").and_then(Value::as_str)
}
