use super::chat_utils::now_unix_ms;
use crate::conversation_store::{
    self, AssistantLine, AssistantStatus, ConversationLine, ToolLine, ToolStatus,
};
use crate::message_phase::AssistantPhase;
use serde_json::Value;

pub struct ToolProgressPersistInput<'a> {
    pub agent_id: &'a str,
    pub conversation_id: &'a str,
    pub request_id: &'a str,
    pub event_kind: &'a str,
    pub tool_call_id: &'a str,
    pub tool_name: Option<&'a str>,
    pub tool_display_name: Option<&'a str>,
    pub tool_description: Option<&'a str>,
    pub tool_icon: Option<&'a str>,
    pub payload: Option<&'a str>,
    pub started_at_unix_ms: Option<i64>,
    pub completed_at_unix_ms: Option<i64>,
}

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

/// Persist a tool-call event during streaming.  Called from the provider
/// stream callback so that tool state survives crashes.
///
/// - `event_kind == "tool_call_started"` → append a `ToolLine` with
///   `status: Pending` before arguments are complete.
/// - `event_kind == "tool_call_done"` → ensure the pending line exists and
///   merge finalized arguments onto it.
/// - `event_kind == "tool_call_exec"` → update the matching `ToolLine`
///   with the output, terminal status, and exact execution timestamps.
pub fn persist_tool_progress_event(
    input: ToolProgressPersistInput<'_>,
    jsonl_backup_enabled: bool,
) -> Result<(), String> {
    let ToolProgressPersistInput {
        agent_id,
        conversation_id,
        request_id,
        event_kind,
        tool_call_id,
        tool_name,
        tool_display_name,
        tool_description,
        tool_icon,
        payload,
        started_at_unix_ms,
        completed_at_unix_ms,
    } = input;

    if tool_call_id.trim().is_empty() {
        return Ok(());
    }
    if !jsonl_backup_enabled {
        return Ok(());
    }

    let ts = now_unix_ms();
    let started_ts = started_at_unix_ms.unwrap_or(ts);
    let completed_ts = completed_at_unix_ms.unwrap_or(ts);
    let line_id = format!("tool-{request_id}-{tool_call_id}");

    match event_kind {
        "tool_call_started" | "tool_call_done" => {
            let name = tool_name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown_tool");
            let args: Value = payload
                .and_then(|p| serde_json::from_str(p).ok())
                .unwrap_or(Value::Null);

            if event_kind == "tool_call_started"
                || !conversation_store::conversation_line_exists(agent_id, conversation_id, &line_id)?
            {
                // First write is append-only so crashes still leave an
                // in-progress tool marker. Later updates merge arguments.
                conversation_store::append_line(agent_id, conversation_store::AppendLineInput {
                    conversation_id: conversation_id.to_string(),
                    line: ConversationLine::Tool(ToolLine {
                        id: line_id.clone(),
                        ts,
                        started_ts,
                        completed_ts: None,
                        request_id: request_id.to_string(),
                        call_id: tool_call_id.to_string(),
                        name: name.to_string(),
                        display_name: tool_display_name.map(str::to_string),
                        description: tool_description.map(str::to_string),
                        icon: tool_icon.map(str::to_string),
                        args: args.clone(),
                        output: None,
                        status: ToolStatus::Pending,
                    }),
                })?;
            }

            if event_kind == "tool_call_done" {
                conversation_store::update_line(agent_id, conversation_store::UpdateLineInput {
                    conversation_id: conversation_id.to_string(),
                    line_id,
                    line: ConversationLine::Tool(ToolLine {
                        id: format!("tool-{request_id}-{tool_call_id}"),
                        ts,
                        started_ts: 0,
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
                })?;
            }

            Ok(())
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

            conversation_store::update_line(agent_id, conversation_store::UpdateLineInput {
                conversation_id: conversation_id.to_string(),
                line_id,
                line: ConversationLine::Tool(ToolLine {
                    id: format!("tool-{request_id}-{tool_call_id}"),
                    ts: completed_ts,
                    started_ts: started_at_unix_ms.unwrap_or(0),
                    completed_ts: Some(completed_ts),
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
            .map_err(|e| e.to_string())
        }
        _ => Ok(()),
    }
}

/// Persist a completed assistant message item in provider order.
pub fn persist_assistant_line(
    agent_id: &str,
    conversation_id: &str,
    request_id: &str,
    response_id: &str,
    phase: Option<AssistantPhase>,
    text: &str,
    jsonl_backup_enabled: bool,
) -> Result<(), String> {
    if !jsonl_backup_enabled {
        return Ok(());
    }
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
        thinking: None,
        thinking_token_count: None,
        status: AssistantStatus::Done,
    });
    conversation_store::append_line(agent_id, conversation_store::AppendLineInput {
        conversation_id: conversation_id.to_string(),
        line,
    })
    .map_err(|e| e.to_string())
}
