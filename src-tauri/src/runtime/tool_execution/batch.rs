use super::types::{ExecutedToolBatch, ExecutedToolRecord};
use serde_json::{Value, json};

pub(super) fn finalize_executed_records(
    provider_kind: &str,
    mut executed_records: Vec<ExecutedToolRecord>,
) -> crate::error::AgentJaxResult<ExecutedToolBatch> {
    let mut tool_results_items = Vec::new();
    let mut executed_tool_call_items = Vec::new();
    let mut timeline_events = Vec::new();
    let mut state_changes = Vec::new();

    executed_records.sort_by_key(|record| record.index);

    for record in executed_records {
        let ExecutedToolRecord {
            call_id,
            name,
            args,
            output_str,
            is_success,
            started_at_unix_ms,
            completed_at_unix_ms,
            duration_ms,
            state_changes: record_state_changes,
            ..
        } = record;

        let output_val: Value =
            serde_json::from_str(&output_str).unwrap_or_else(|_| Value::String(output_str.clone()));
        timeline_events.push(json!({
            "type": "toolCall",
            "callId": call_id.clone(),
            "name": name.clone(),
            "arguments": args.clone(),
            "output": output_val,
            "status": if is_success { "success" } else { "failed" },
            "startedAtUnixMs": started_at_unix_ms,
            "completedAtUnixMs": completed_at_unix_ms,
            "durationMs": duration_ms
        }));

        let tool_input_item = crate::provider_api::build_tool_result_input_item(
            provider_kind,
            &call_id,
            &output_str,
        )?;
        tool_results_items.push(tool_input_item);
        state_changes.extend(record_state_changes);

        executed_tool_call_items.push(json!({
            "type": "function_call",
            "call_id": call_id,
            "name": name,
            "arguments": serde_json::to_string(&args).unwrap_or_else(|_| "{}".to_string()),
        }));
    }

    Ok(ExecutedToolBatch {
        tool_results_items,
        executed_tool_call_items,
        timeline_events,
        state_changes,
    })
}
