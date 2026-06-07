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
    _call_id: &str,
    tool_name: &str,
    arguments: &Value,
    outputs: &[Value],
    extra_note: &str,
) -> Value {
    let output_value = if outputs.is_empty() {
        Value::Null
    } else if outputs.len() == 1 {
        outputs[0].clone()
    } else {
        Value::Array(outputs.to_vec())
    };

    let extra_line = if extra_note.is_empty() {
        String::new()
    } else {
        format!("\n         Note: {extra_note}")
    };

    let note = format!(
        "━━━ Archived Tool Call ━━━\n\
         Tool: {tool_name} (currently unavailable)\n\
         Arguments: {arguments}\n\
         Output: {output}{extra_line}\n\
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

    // ── Third pass: emit output ───────────────────────────────────────
    //
    //    KEY: Archived notes are appended at the END rather than inserted
    //    inline.  When a single assistant message has multiple tool calls
    //    (e.g. kb_index + get_system_time) and only *some* are disabled,
    //    inserting an archived note between function_call items would
    //    cause `input_items_to_messages` (Chat Completions conversion)
    //    to flush the pending assistant prematurely, breaking the
    //    function_call / function_call_output pairing and producing a
    //    400 "insufficient tool messages" error.
    //
    //    By collecting archived notes and appending them at the end,
    //    the remaining function_call → function_call_output pairs stay
    //    contiguous and the Chat Completions conversion keeps them
    //    together in a single assistant message with matching tool
    //    messages.

    let mut archived_notes: Vec<Value> = Vec::new();
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

        if let Some((tool_name, arguments)) = archived_calls.get(call_id) {
            // Build the archived note once per call_id.
            if !emitted_call_ids.contains(call_id) {
                let raw_outputs = outputs_by_call_id.get(call_id).cloned().unwrap_or_default();
                let outputs: Vec<Value> = raw_outputs
                    .iter()
                    .filter_map(|item| item.get("output").cloned())
                    .collect();

                // Determine extra note.
                let is_orphaned_call = !call_ids_with_output.contains(call_id);
                let is_orphaned_output = !call_ids_with_call.contains(call_id);
                let extra = if tool_name.is_empty() {
                    " (tool call was compacted by LCM — use lcm_expand to recover)"
                } else if is_orphaned_call {
                    " (output was compacted by LCM — use lcm_expand to recover)"
                } else if is_orphaned_output {
                    " (call was compacted by LCM — use lcm_expand to recover)"
                } else {
                    ""
                };
                let display_name: &str = if tool_name.is_empty() { "(unknown)" } else { tool_name };
                archived_notes.push(build_archived_tool_note(
                    call_id,
                    display_name,
                    arguments,
                    &outputs,
                    extra,
                ));
                emitted_call_ids.insert(call_id.to_string());
            }
            // Always skip the original item — it's been archived.
            continue;
        }

        // Not in archived_calls — preserve as-is.
        output.push(item);
    }

    // Append all archived notes at the end so they don't break
    // function_call / function_call_output pairing.
    output.append(&mut archived_notes);

    output
}
