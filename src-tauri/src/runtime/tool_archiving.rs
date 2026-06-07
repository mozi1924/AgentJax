use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Dummy tool name used to carry archived context as valid function_call/output pairs.
const CARRIER_TOOL: &str = "_archived_tool";

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

/// Build the `function_call` item for the carrier tool, reusing the
/// original `call_id` so that the Chat Completions conversion keeps
/// this function_call grouped with other tool calls from the same
/// assistant message.
fn build_carrier_function_call(
    original_call_id: &str,
    tool_name: &str,
    arguments: &Value,
    note: &str,
) -> Value {
    let arch_args = json!({
        "original_tool": tool_name,
        "original_call_id": original_call_id,
        "original_arguments": arguments,
        "note": note,
    });
    json!({
        "type": "function_call",
        "call_id": original_call_id,
        "name": CARRIER_TOOL,
        "arguments": to_compact_json(&arch_args, 4000),
    })
}

/// Build the `function_call_output` item for the carrier tool, reusing
/// the original `call_id` so the Chat Completions conversion pairs it
/// with the renamed function_call above.
fn build_carrier_function_call_output(
    original_call_id: &str,
    tool_name: &str,
    arguments: &Value,
    outputs: &[Value],
    note: &str,
) -> Value {
    let output_value = if outputs.is_empty() {
        Value::Null
    } else if outputs.len() == 1 {
        outputs[0].clone()
    } else {
        Value::Array(outputs.to_vec())
    };
    let arch_output = json!({
        "original_tool": tool_name,
        "original_arguments": arguments,
        "original_output": output_value,
        "note": note,
    });
    json!({
        "type": "function_call_output",
        "call_id": original_call_id,
        "output": to_compact_json(&arch_output, 8000),
    })
}

pub(crate) fn archive_unavailable_historical_tool_calls(
    input_items: Vec<Value>,
    active_tool_names: &HashSet<String>,
) -> Vec<Value> {
    // ── First pass: collect all tool items and identify mismatches.
    //
    //    Three categories of problematic tool calls:
    //
    //    1. Unavailable tools — tool name not in active_tool_names
    //       (user disabled the tool in settings).
    //    2. Orphaned function_call — has no matching function_call_output
    //       (LCM compaction removed the tool result but left the
    //       assistant message with tool_calls_json intact).
    //    3. Orphaned function_call_output — has no matching function_call
    //       (LCM compaction removed the assistant message but left the
    //       tool result, because standalone tool messages are excluded
    //       from compaction).
    //
    //    All three produce invalid Chat Completions messages and must be
    //    converted to user-role archived notes.
    let mut outputs_by_call_id = HashMap::<String, Vec<Value>>::new();
    let mut call_ids_with_output = HashSet::new();
    let mut call_ids_with_call = HashSet::new();

    for item in &input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        if matches!(
            item_type,
            "function_call_output" | "custom_tool_call_output"
        ) {
            if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                let cid = call_id.to_string();
                outputs_by_call_id
                    .entry(cid.clone())
                    .or_default()
                    .push(item.clone());
                call_ids_with_output.insert(cid);
            }
            continue;
        }

        if matches!(item_type, "function_call" | "custom_tool_call") {
            if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                call_ids_with_call.insert(call_id.to_string());
            }
        }
    }

    let mut archived_calls: HashMap<String, (String, Value)> = HashMap::new();

    for item in &input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

        // ── Handle orphaned function_call_output (category 3) ─────────
        if matches!(
            item_type,
            "function_call_output" | "custom_tool_call_output"
        ) {
            if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                if !call_ids_with_call.contains(call_id) {
                    archived_calls
                        .entry(call_id.to_string())
                        .or_insert_with(|| (String::new(), json!({})));
                }
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

        // ── Category 1: tool is unavailable ─────────────────────────
        let tool_unavailable =
            active_tool_names.is_empty() || !active_tool_names.contains(name);

        // ── Category 2: orphaned call (no matching output) ──────────
        let orphaned = !call_ids_with_output.contains(call_id);

        if tool_unavailable || orphaned {
            archived_calls
                .entry(call_id.to_string())
                .or_insert_with(|| (name.to_string(), parse_tool_call_item_arguments(item)));
        }
    }

    if archived_calls.is_empty() {
        log::debug!(
            "Archive: no items to archive (active_tool_names=[{}], input items={})",
            active_tool_names.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
            input_items.len(),
        );
        return input_items;
    }

    log::info!(
        "Archive: archiving {} historical tool call(s) — names=[{}], call_ids=[{}], reasons=[{}]",
        archived_calls.len(),
        archived_calls.values().map(|(name, _)| name.as_str()).collect::<Vec<_>>().join(", "),
        archived_calls.keys().map(|s| s.as_str()).collect::<Vec<_>>().join(", "),
        archived_calls.keys().map(|cid| {
            let (name, _) = &archived_calls[cid];
            let unavailable = active_tool_names.is_empty() || !active_tool_names.contains(name.as_str());
            let orphaned_call = !call_ids_with_output.contains(cid);
            let orphaned_output = !call_ids_with_call.contains(cid);
            if unavailable && orphaned_call { format!("{name}: unavailable+orphaned-call") }
            else if unavailable { format!("{name}: unavailable") }
            else if orphaned_call { format!("{name}: orphaned-call") }
            else if orphaned_output { format!("{name}: orphaned-output") }
            else { format!("{name}: unknown") }
        }).collect::<Vec<_>>().join("; "),
    );

    // ── Third pass: rewrite archived items in-place ───────────────────
    //
    //    KEY INSIGHT: instead of inserting new items (which disrupts the
    //    ordering and breaks Chat Completions pairing), we MODIFY the
    //    original items in-place:
    //
    //    - function_call:  name → _archived_tool, args wrapped, call_id kept
    //    - function_call_output: output wrapped, call_id kept
    //
    //    Because the call_id is unchanged and the item positions are
    //    unchanged, the Chat Completions conversion in
    //    `input_items_to_messages` keeps them correctly interleaved
    //    with other tool calls from the same assistant message group.

    let mut emitted_call_ids = HashSet::new();
    let mut output = Vec::with_capacity(input_items.len());

    for item in input_items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");

        if matches!(item_type, "function_call" | "custom_tool_call") {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if !call_id.is_empty()
                && let Some((tool_name, arguments)) = archived_calls.get(call_id)
            {
                let is_orphaned_call = !call_ids_with_output.contains(call_id);
                let note = if tool_name.is_empty() {
                    "Tool call was compacted by LCM — use lcm_expand to recover details."
                } else if is_orphaned_call {
                    "Output was compacted by LCM — use lcm_expand to recover."
                } else {
                    "Tool is currently disabled in agent settings."
                };
                let display_name: &str =
                    if tool_name.is_empty() { "(unknown)" } else { tool_name };

                output.push(build_carrier_function_call(
                    call_id,
                    display_name,
                    arguments,
                    note,
                ));
                emitted_call_ids.insert(call_id.to_string());
                continue;
            }
            output.push(item);
            continue;
        }

        if matches!(
            item_type,
            "function_call_output" | "custom_tool_call_output"
        ) {
            let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
            if !call_id.is_empty() && archived_calls.contains_key(call_id) {
                let (tool_name, arguments) = &archived_calls[call_id];
                let raw_outputs =
                    outputs_by_call_id.get(call_id).cloned().unwrap_or_default();
                let outputs: Vec<Value> = raw_outputs
                    .iter()
                    .filter_map(|o| o.get("output").cloned())
                    .collect();
                let is_orphaned_output = !call_ids_with_call.contains(call_id);
                let note = if tool_name.is_empty() {
                    "Tool call was compacted by LCM."
                } else if is_orphaned_output {
                    "Call was compacted by LCM — use lcm_expand to recover."
                } else if emitted_call_ids.contains(call_id) {
                    "Tool is currently disabled in agent settings."
                } else {
                    "Tool is currently disabled."
                };
                let display_name: &str =
                    if tool_name.is_empty() { "(unknown)" } else { tool_name };

                output.push(build_carrier_function_call_output(
                    call_id,
                    display_name,
                    arguments,
                    &outputs,
                    note,
                ));
                emitted_call_ids.insert(call_id.to_string());
                continue;
            }
            output.push(item);
            continue;
        }

        output.push(item);
    }

    output
}
