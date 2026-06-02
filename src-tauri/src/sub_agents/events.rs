//! Sub-agent event types — bridge between sub-agent runner and frontend streaming.
//!
//! SubAgentEvents are emitted by the sub-agent runner and mapped to
//! `ChatStreamEvent` variants for the frontend. The `agent_id` field
//! on `ChatStreamEvent` distinguishes sub-agent events from main-agent events.

use serde::Serialize;
use serde_json::Value;

// ── SubAgentEvent ─────────────────────────────────────────────────────────────

/// Events emitted by a sub-agent during its lifecycle.
///
/// These are converted to `ChatStreamEvent` variants by the chat stream
/// observer so the frontend can track sub-agent progress in real time.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)] // HopCompleted variant reserved for future use
pub enum SubAgentEvent {
    /// Sub-agent has been spawned and is about to start running.
    Spawned {
        agent_id: String,
        subagent_type: String,
        parent_request_id: String,
    },

    /// Sub-agent has started executing.
    Started { agent_id: String },

    /// Textual progress update from the sub-agent.
    Progress {
        agent_id: String,
        text: String,
        turns_completed: usize,
        turns_remaining: usize,
    },

    /// A tool call started within the sub-agent.
    ToolCallStarted {
        agent_id: String,
        call_id: String,
        tool_name: String,
    },

    /// A tool call completed within the sub-agent.
    ToolCallCompleted {
        agent_id: String,
        call_id: String,
        tool_name: String,
        tool_status: String,
    },

    /// A hop completed within the sub-agent's turn loop.
    HopCompleted {
        agent_id: String,
        hop_index: usize,
        text: Option<String>,
    },

    /// Sub-agent completed successfully.
    Completed {
        agent_id: String,
        result: Value,
        duration_ms: u64,
    },

    /// Sub-agent failed with an error.
    Failed {
        agent_id: String,
        error: String,
        duration_ms: u64,
    },

    /// Sub-agent was cancelled.
    Cancelled {
        agent_id: String,
        reason: String,
    },
}

/// Map a `SubAgentEvent` to the corresponding `ChatStreamEvent` payload.
///
/// This is called by the chat stream handler's forwarding task so sub-agent
/// lifecycle events reach the frontend as standard stream events with an
/// `agentId` field set.
pub fn sub_agent_event_to_chat_stream_event(
    event: &SubAgentEvent,
    request_id: &str,
    event_index: &mut u64,
) -> crate::commands::chat::chat_events::ChatStreamEvent {
    let mut chat_event = crate::commands::chat::chat_events::ChatStreamEvent {
        request_id: request_id.to_string(),
        event_index: *event_index,
        kind: event.kind().to_string(),
        delta: None,
        response_id: None,
        conversation_id: None,
        conversation_title: None,
        error: None,
        tool_call_id: None,
        tool_name: None,
        tool_display_name: None,
        tool_description: None,
        tool_icon: None,
        tool_arguments: None,
        tool_output: None,
        tool_status: None,
        tool_started_ts: None,
        tool_completed_ts: None,
        tool_duration_ms: None,
        context_token_count: None,
        phase: None,
        agent_id: Some(event.agent_id().to_string()),
    };

    *event_index += 1;

    match event {
        SubAgentEvent::Spawned { subagent_type, .. } => {
            chat_event.tool_name = Some(format!("sub_agent ({})", subagent_type));
        }
        SubAgentEvent::Progress { text, turns_completed, turns_remaining, .. } => {
            chat_event.delta = Some(format!(
                "[turns {}/{} remaining] {}",
                turns_completed, turns_remaining, text
            ));
        }
        SubAgentEvent::ToolCallStarted { call_id, tool_name, .. } => {
            chat_event.tool_call_id = Some(call_id.clone());
            chat_event.tool_name = Some(tool_name.clone());
        }
        SubAgentEvent::ToolCallCompleted { call_id, tool_name, tool_status, .. } => {
            chat_event.tool_call_id = Some(call_id.clone());
            chat_event.tool_name = Some(tool_name.clone());
            chat_event.tool_status = Some(tool_status.clone());
        }
        SubAgentEvent::HopCompleted { hop_index, text, .. } => {
            chat_event.delta = text.clone();
            chat_event.tool_name = Some(format!("hop_{}", hop_index));
        }
        SubAgentEvent::Completed { result, duration_ms, .. } => {
            chat_event.delta = serde_json::to_string(result)
                .ok()
                .or_else(|| Some("completed".to_string()));
            chat_event.tool_duration_ms = Some(*duration_ms);
        }
        SubAgentEvent::Failed { error, duration_ms, .. } => {
            chat_event.error = Some(error.clone());
            chat_event.tool_duration_ms = Some(*duration_ms);
        }
        SubAgentEvent::Cancelled { reason, .. } => {
            chat_event.error = Some(reason.clone());
        }
        _ => {}
    }

    chat_event
}

impl SubAgentEvent {
    /// Returns the agent_id for this event.
    pub fn agent_id(&self) -> &str {
        match self {
            SubAgentEvent::Spawned { agent_id, .. }
            | SubAgentEvent::Started { agent_id }
            | SubAgentEvent::Progress { agent_id, .. }
            | SubAgentEvent::ToolCallStarted { agent_id, .. }
            | SubAgentEvent::ToolCallCompleted { agent_id, .. }
            | SubAgentEvent::HopCompleted { agent_id, .. }
            | SubAgentEvent::Completed { agent_id, .. }
            | SubAgentEvent::Failed { agent_id, .. }
            | SubAgentEvent::Cancelled { agent_id, .. } => agent_id,
        }
    }

    /// Returns the event kind string for the `ChatStreamEvent.kind` field.
    pub fn kind(&self) -> &'static str {
        match self {
            SubAgentEvent::Spawned { .. } => "sub_agent_spawned",
            SubAgentEvent::Started { .. } => "sub_agent_started",
            SubAgentEvent::Progress { .. } => "sub_agent_progress",
            SubAgentEvent::ToolCallStarted { .. } => "sub_agent_tool_call_started",
            SubAgentEvent::ToolCallCompleted { .. } => "sub_agent_tool_call_done",
            SubAgentEvent::HopCompleted { .. } => "sub_agent_hop_completed",
            SubAgentEvent::Completed { .. } => "sub_agent_completed",
            SubAgentEvent::Failed { .. } => "sub_agent_failed",
            SubAgentEvent::Cancelled { .. } => "sub_agent_cancelled",
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_event_agent_id() {
        let event = SubAgentEvent::Started {
            agent_id: "agent_1".to_string(),
        };
        assert_eq!(event.agent_id(), "agent_1");
    }

    #[test]
    fn test_event_kind() {
        let event = SubAgentEvent::Progress {
            agent_id: "a".to_string(),
            text: "working".to_string(),
            turns_completed: 1,
            turns_remaining: 4,
        };
        assert_eq!(event.kind(), "sub_agent_progress");
    }

    #[test]
    fn test_event_serialization() {
        let event = SubAgentEvent::Completed {
            agent_id: "agent_1".to_string(),
            result: serde_json::json!({"ok": true}),
            duration_ms: 1500,
        };
        let json = serde_json::to_string(&event).unwrap();
        assert!(json.contains("agent_1"));
        assert!(json.contains("completed"));
    }
}
