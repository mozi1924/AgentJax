use super::chat_utils::now_unix_ms;
use crate::conversation_store::{
    self, AssistantLine, AssistantStatus, ConversationLine, ToolLine, ToolStatus,
};
use crate::message_phase::AssistantPhase;
use serde_json::Value;

/// Persist a tool-call event during streaming.  Called from the provider
/// stream callback so that tool state survives crashes.
///
/// - `event_kind == "tool_call_done"` → append a `ToolLine` with
///   `status: Pending` (no output yet).
/// - `event_kind == "tool_call_exec"` → update the matching `ToolLine`
///   with the output and set a terminal success/failure status.

fn is_successful_tool_output(output: &Value) -> bool {
    match output {
        Value::Object(map) => {
            if map.get("ok").and_then(Value::as_bool) == Some(false) {
                return false;
            }
            !map.contains_key("error")
        }
        _ => true,
    }
}

pub fn persist_tool_progress_event(
    conversation_id: &str,
    request_id: &str,
    event_kind: &str,
    tool_call_id: &str,
    tool_name: Option<&str>,
    tool_display_name: Option<&str>,
    tool_description: Option<&str>,
    tool_icon: Option<&str>,
    payload: Option<&str>,
) -> Result<(), String> {
    if tool_call_id.trim().is_empty() {
        return Ok(());
    }

    let ts = now_unix_ms();
    let line_id = format!("tool-{request_id}-{tool_call_id}");

    match event_kind {
        "tool_call_done" => {
            let name = tool_name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown_tool");
            let args: Value = payload
                .and_then(|p| serde_json::from_str(p).ok())
                .unwrap_or(Value::Null);

            conversation_store::append_line(conversation_store::AppendLineInput {
                conversation_id: conversation_id.to_string(),
                line: ConversationLine::Tool(ToolLine {
                    id: line_id.clone(),
                    ts,
                    started_ts: ts,
                    completed_ts: None,
                    request_id: request_id.to_string(),
                    call_id: tool_call_id.to_string(),
                    name: name.to_string(),
                    display_name: tool_display_name.map(str::to_string),
                    description: tool_description.map(str::to_string),
                    icon: tool_icon.map(str::to_string),
                    args,
                    output: None,
                    status: ToolStatus::Pending,
                }),
            })
        }
        "tool_call_exec" => {
            let output: Value = payload
                .and_then(|p| serde_json::from_str(p).ok())
                .unwrap_or(Value::Null);
            let status = if is_successful_tool_output(&output) {
                ToolStatus::Done
            } else {
                ToolStatus::Failed
            };

            conversation_store::update_line(conversation_store::UpdateLineInput {
                conversation_id: conversation_id.to_string(),
                line_id,
                line: ConversationLine::Tool(ToolLine {
                    id: format!("tool-{request_id}-{tool_call_id}"),
                    ts,
                    started_ts: 0,
                    completed_ts: Some(ts),
                    request_id: request_id.to_string(),
                    call_id: tool_call_id.to_string(),
                    name: tool_name
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .unwrap_or("unknown_tool")
                        .to_string(),
                    display_name: tool_display_name.map(str::to_string),
                    description: tool_description.map(str::to_string),
                    icon: tool_icon.map(str::to_string),
                    args: Value::Null, // preserved from the Pending entry; not overwritten
                    output: Some(output),
                    status,
                }),
            })
        }
        _ => Ok(()),
    }
}

/// Persist a completed assistant message item in provider order.
pub fn persist_assistant_line(
    conversation_id: &str,
    request_id: &str,
    response_id: &str,
    phase: Option<AssistantPhase>,
    text: &str,
) -> Result<(), String> {
    let text = text.trim().to_string();
    if text.is_empty() {
        return Ok(());
    }
    let ts = now_unix_ms();
    let line = ConversationLine::Assistant(AssistantLine {
        id: format!("asst-{request_id}-{}", ts),
        ts,
        request_id: request_id.to_string(),
        response_id: response_id.to_string(),
        phase,
        text,
        status: AssistantStatus::Done,
    });
    conversation_store::append_line(conversation_store::AppendLineInput {
        conversation_id: conversation_id.to_string(),
        line,
    })
}
