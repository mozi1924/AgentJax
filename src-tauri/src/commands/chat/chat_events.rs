use crate::providers::types::ProviderStreamEvent;
use serde::Serialize;
use tauri::Emitter;

#[derive(Debug, Serialize, Clone)]
#[serde(rename_all = "camelCase")]
pub struct ChatStreamEvent {
    pub request_id: String,
    pub event_index: u64,
    pub kind: String,
    pub delta: Option<String>,
    pub response_id: Option<String>,
    pub conversation_id: Option<String>,
    pub conversation_title: Option<String>,
    pub error: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_display_name: Option<String>,
    pub tool_description: Option<String>,
    pub tool_icon: Option<String>,
    pub tool_arguments: Option<String>,
    pub tool_output: Option<String>,
    /// When `kind == "delta"`, signals whether this text belongs to the
    /// commentary phase or the final answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
}

pub fn next_event_index(current: &mut u64) -> u64 {
    *current += 1;
    *current
}

pub fn emit_mapped_stream_event(
    window: &tauri::Window,
    request_id: &str,
    conversation_id: &str,
    event_index: &mut u64,
    event: ProviderStreamEvent,
) -> Result<(), String> {
    let mut chat_event = ChatStreamEvent {
        request_id: request_id.to_string(),
        event_index: next_event_index(event_index),
        kind: "".to_string(),
        delta: None,
        response_id: None,
        conversation_id: Some(conversation_id.to_string()),
        conversation_title: None,
        error: None,
        tool_call_id: None,
        tool_name: None,
        tool_display_name: None,
        tool_description: None,
        tool_icon: None,
        tool_arguments: None,
        tool_output: None,
        phase: None,
    };

    match event {
        ProviderStreamEvent::ReasoningStarted => {
            chat_event.kind = "thinking".to_string();
        }
        ProviderStreamEvent::OutputTextStarted => {
            chat_event.kind = "output_started".to_string();
        }
        ProviderStreamEvent::OutputTextDelta { delta, phase } => {
            chat_event.kind = "delta".to_string();
            chat_event.delta = Some(delta);
            chat_event.phase = phase.map(|phase| phase.as_str().to_string());
        }
        ProviderStreamEvent::ToolCallStarted {
            item_id: _,
            call_id,
            name,
            presentation,
        } => {
            chat_event.kind = "tool_call_started".to_string();
            chat_event.tool_call_id = Some(call_id);
            chat_event.tool_name = Some(name);
            if let Some(presentation) = presentation {
                if !presentation.display_name.trim().is_empty() {
                    chat_event.tool_display_name = Some(presentation.display_name);
                }
                if !presentation.description.trim().is_empty() {
                    chat_event.tool_description = Some(presentation.description);
                }
                chat_event.tool_icon = presentation.icon;
            }
        }
        ProviderStreamEvent::ToolCallArgumentsDelta {
            item_id: _,
            call_id,
            delta,
        } => {
            chat_event.kind = "tool_call_delta".to_string();
            chat_event.tool_call_id = Some(call_id);
            chat_event.delta = Some(delta);
        }
        ProviderStreamEvent::ToolCallCompleted {
            item_id: _,
            call_id,
            name,
            arguments,
            presentation,
        } => {
            chat_event.kind = "tool_call_done".to_string();
            chat_event.tool_call_id = Some(call_id);
            chat_event.tool_name = Some(name);
            chat_event.tool_arguments = Some(arguments);
            if let Some(presentation) = presentation {
                if !presentation.display_name.trim().is_empty() {
                    chat_event.tool_display_name = Some(presentation.display_name);
                }
                if !presentation.description.trim().is_empty() {
                    chat_event.tool_description = Some(presentation.description);
                }
                chat_event.tool_icon = presentation.icon;
            }
        }
        ProviderStreamEvent::ToolCallExecuted {
            call_id,
            name,
            output,
            presentation,
        } => {
            chat_event.kind = "tool_call_exec".to_string();
            chat_event.tool_call_id = Some(call_id);
            chat_event.tool_name = Some(name);
            chat_event.tool_output = Some(output);
            if let Some(presentation) = presentation {
                if !presentation.display_name.trim().is_empty() {
                    chat_event.tool_display_name = Some(presentation.display_name);
                }
                if !presentation.description.trim().is_empty() {
                    chat_event.tool_description = Some(presentation.description);
                }
                chat_event.tool_icon = presentation.icon;
            }
        }
        ProviderStreamEvent::AssistantMessageCompleted { .. } => {
            return Ok(());
        }
        ProviderStreamEvent::HopAssistantText {
            text,
            phase,
            response_id,
        } => {
            chat_event.kind = "assistant_message".to_string();
            chat_event.delta = Some(text);
            chat_event.phase = phase.map(|phase| phase.as_str().to_string());
            if !response_id.trim().is_empty() {
                chat_event.response_id = Some(response_id);
            }
        }
        ProviderStreamEvent::ResponseCompleted => {
            return Ok(());
        }
    };

    window
        .emit("chat_stream_event", chat_event)
        .map_err(|e| format!("Failed to emit stream event: {e}"))
}
