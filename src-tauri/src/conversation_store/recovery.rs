use super::file_io::read_conversation_file;
use super::paths::{conversation_messages_path, conversation_metadata_path};
use super::types::{AssistantStatus, ConversationLine, ToolStatus};
use serde_json::{json, Value};

/// Check whether the most recent request in this conversation completed
/// cleanly.  Returns `Some(developer_note)` if recovery is needed, or
/// `None` if everything is fine.
///
/// A request is "complete" when its last line is an `assistant` with
/// `status: Done`.  Any other terminal state needs recovery.
pub fn build_recovery_developer_note(conversation_id: &str) -> Result<Option<Value>, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };

    // Find the most recent request.
    let last_request_id = data
        .lines
        .iter()
        .rev()
        .find_map(|line| line.request_id().map(ToOwned::to_owned));

    let Some(last_request_id) = last_request_id else {
        return Ok(None);
    };

    // Gather the terminal state of this request.
    let mut has_user = false;
    let mut unresolved: Vec<Value> = Vec::new();
    let mut completed: Vec<Value> = Vec::new();
    let mut has_final_answer_done = false;
    let mut has_assistant_draft = false;

    for line in &data.lines {
        if line.request_id() != Some(&last_request_id) {
            continue;
        }

        match line {
            ConversationLine::User(_) => has_user = true,
            ConversationLine::Tool(t) => match t.status {
                ToolStatus::Pending => {
                    unresolved.push(json!({
                        "call_id": t.call_id,
                        "tool": t.name,
                        "arguments": t.args,
                    }));
                }
                ToolStatus::Done | ToolStatus::Failed => {
                    completed.push(json!({
                        "call_id": t.call_id,
                        "tool": t.name,
                        "arguments": t.args,
                    }));
                }
            },
            ConversationLine::Assistant(a) => {
                if !a.is_final_or_unknown() {
                    continue;
                }
                match a.status {
                    AssistantStatus::Draft => has_assistant_draft = true,
                    AssistantStatus::Done => has_final_answer_done = true,
                }
            }
        }
    }

    if !has_user {
        return Ok(None);
    }

    // Already done — nothing to recover.
    if has_final_answer_done && unresolved.is_empty() {
        return Ok(None);
    }

    // ── Build recovery note ───────────────────────────────────────────
    let interruption_reason = if unresolved.is_empty() && has_assistant_draft {
        "Assistant response was interrupted mid-generation and never completed.".to_string()
    } else if !unresolved.is_empty() {
        format!(
            "{} tool call(s) were issued but never executed. The assistant response was interrupted before these tools could run.",
            unresolved.len()
        )
    } else {
        "The assistant finished its last tool call but never produced a final response.".to_string()
    };

    let payload = json!({
        "recovery_type": "unfinished_turn",
        "request_id": last_request_id,
        "interruption_reason": interruption_reason,
        "assistant_message_missing": !has_final_answer_done,
        "completed_tool_calls": completed,
        "unresolved_tool_calls": unresolved,
    });

    let mut note_parts = vec![format!(
        "RECOVERY_CONTEXT {}",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    )];

    note_parts.push("The previous request was interrupted before completing. ".to_string());

    if !completed.is_empty() {
        note_parts.push(format!(
            "{} tool call(s) already completed successfully — do NOT re-execute them. ",
            completed.len()
        ));
    }
    if !unresolved.is_empty() {
        note_parts.push(format!(
            "{} tool call(s) were issued but never resolved — you may re-issue them if still needed, but first check whether the information they would have returned is already available in the conversation context above. ",
            unresolved.len()
        ));
    }
    if !has_final_answer_done {
        note_parts.push(
            "No final assistant response was saved. Continue from the current state and produce a complete answer. "
                .to_string(),
        );
    }
    note_parts.push(
        "Do NOT repeat already-completed work. Use the conversation context above to understand what has already been done."
            .to_string(),
    );

    Ok(Some(json!({
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": note_parts.concat()
        }]
    })))
}
