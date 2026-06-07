//! Archived tool — a dummy tool that carries archived/historical tool call
//! context as a valid function_call / function_call_output pair.
//!
//! When a tool is disabled, the archiver replaces its historical tool calls
//! with `_archived_tool` function_call + function_call_output pairs.  This
//! preserves the Chat Completions interleaving invariant
//! (assistant(tool_calls) → tool messages) that would be broken by inserting
//! a user-role note between function_call items.

use crate::error::AgentJaxResult;
use crate::tools::{Tool, ToolExecutionContext};
use serde_json::{Value, json};

/// The canonical name exposed to the model.
pub const ARCHIVED_TOOL_NAME: &str = "_archived_tool";

pub struct ArchivedTool;

#[async_trait::async_trait]
impl Tool for ArchivedTool {
    fn name(&self) -> &'static str {
        ARCHIVED_TOOL_NAME
    }

    fn display_name(&self) -> &'static str {
        "Archived Context"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("Archive")
    }

    fn description(&self) -> &'static str {
        "🚫 DO NOT INVOKE — INTERNAL DUMMY TOOL.  This tool exists solely to \
         carry archived/historical tool call context that is no longer \
         available.  It preserves Chat Completions message structure for \
         context that has been compacted or for tools that have been \
         disabled.  You should NEVER call this tool — read the archived \
         context from the conversation history instead.  If you call it, \
         you will receive a warning and the call will be ignored."
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "description": "🚫 INTERNAL — DO NOT INVOKE.  This schema exists only so that archived tool calls can be represented as valid function_call / function_call_output pairs.  The data here is historical context, not an action for you to perform.",
            "properties": {
                "original_tool": {
                    "type": "string",
                    "description": "Name of the original (now unavailable) tool that was called in the past."
                },
                "original_call_id": {
                    "type": "string",
                    "description": "Original call_id from the historical tool call — for traceability only."
                },
                "original_arguments": {
                    "description": "Arguments that were passed to the original tool call — historical record, do not act on."
                },
                "note": {
                    "type": "string",
                    "description": "Why this tool call was archived (disabled, compacted, etc.)."
                }
            },
            "required": ["original_tool"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        _context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let original = arguments
            .get("original_tool")
            .and_then(|v| v.as_str())
            .unwrap_or("(unknown)");
        Ok(json!({
            "ok": false,
            "warning": "🚫 _archived_tool is an internal context-carrier dummy tool and must never be invoked directly.",
            "original_tool": original,
            "guidance": "This call was archived because the original tool is no longer available or its context was compacted. The archived output is already present in the conversation history — use that information directly. Do not attempt to re-execute archived tool calls.",
            "action": "ignore"
        }))
    }
}
