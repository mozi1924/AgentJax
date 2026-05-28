use crate::conversation_store::{
    AssistantLine, AssistantStatus, ConversationLine, ToolLine, UserLine,
};
use crate::time_context::{attach_tool_output_time_metadata, render_timed_message};
use serde_json::{Value, json};

/// Convert stored conversation lines into raw Responses-API input items.
///
/// Each line kind maps to a specific item shape so the rest of the pipeline
/// can operate on a predictable, model-facing representation.
pub(super) fn build_context_items(lines: &[ConversationLine]) -> Vec<Value> {
    let mut input_items = Vec::new();

    for line in lines {
        match line {
            ConversationLine::User(user) => {
                input_items.push(build_user_input_item(user));
            }
            ConversationLine::Assistant(assistant) => {
                if assistant.status != AssistantStatus::Done || assistant.text.trim().is_empty() {
                    continue;
                }
                input_items.push(build_assistant_input_item(assistant));
            }
            ConversationLine::Tool(tool) => {
                input_items.extend(build_tool_input_items(tool));
            }
        }
    }

    input_items
}

fn build_user_input_item(line: &UserLine) -> Value {
    json!({
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": render_timed_message("User message", line.ts, &line.text)
        }]
    })
}

fn build_assistant_input_item(line: &AssistantLine) -> Value {
    let mut item = json!({
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{
            "type": "output_text",
            "text": render_timed_message("Assistant message", line.ts, &line.text),
            "annotations": []
        }]
    });

    if let Some(phase) = line.phase {
        item["phase"] = Value::String(phase.as_str().to_string());
    }

    item
}

fn build_tool_input_items(line: &ToolLine) -> Vec<Value> {
    let mut items = Vec::with_capacity(2);
    let call_id = &line.call_id;
    let arguments = if let Some(args) = line.args.as_str() {
        args.to_string()
    } else {
        serde_json::to_string(&line.args).unwrap_or_else(|_| "{}".to_string())
    };

    items.push(json!({
        "type": "function_call",
        "call_id": call_id,
        "name": line.name,
        "arguments": arguments,
    }));

    if let Some(output) = &line.output {
        let timed_output = attach_tool_output_time_metadata(
            output,
            line.started_at_unix_ms(),
            line.completed_at_unix_ms(),
            line.completed_at_unix_ms()
                .map(|completed_at| completed_at.saturating_sub(line.started_at_unix_ms()) as u64),
        );
        items.push(json!({
            "type": "function_call_output",
            "call_id": call_id,
            "output": serde_json::to_string(&timed_output).unwrap_or_else(|_| "{}".to_string()),
        }));
    }

    items
}
