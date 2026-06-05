//! `lcm_expand` — expand summary nodes back to original messages.
//!
//! Corresponds to Appendix C.1 of the LCM paper.
//!
//! **IMPORTANT**: This tool is restricted to sub-agents only.
//! The main agent cannot call `lcm_expand` directly — it must delegate
//! to an ephemeral sub-agent via `sub_agent(action='spawn', ...)`. This
//! prevents uncontrolled context growth in the primary interaction loop (§2.4).
//!
//! When called by a sub-agent, it recursively traverses the summary DAG
//! to recover all original messages covered by the specified summary node.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::lcm::store::LcmStore;
use crate::lcm::types::LcmId;
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct LcmExpandTool {
    store: Arc<LcmStore>,
}

impl LcmExpandTool {
    pub fn new(store: Arc<LcmStore>) -> Self {
        Self { store }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LcmExpandArgs {
    /// The summary node ID to expand.
    summary_id: String,
    /// Maximum number of messages to return (to prevent context flooding).
    #[serde(default = "default_max_messages")]
    max_messages: usize,
}

fn default_max_messages() -> usize {
    50
}

#[async_trait::async_trait]
impl Tool for LcmExpandTool {
    fn name(&self) -> &'static str {
        "lcm_expand"
    }

    fn description(&self) -> &'static str {
        "Expand a summary node back into its original messages. \
         ⚠️ RESTRICTED: only available in ephemeral sub-agents, not \
         in the main conversation loop. The main agent should delegate \
         expansion work via sub_agent(action='spawn', subagentType='explore'). \
         Recursively traverses the summary DAG to recover all original \
         messages covered by the specified summary."
    }

    fn display_name(&self) -> &'static str {
        "LCM Expand"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("unfold")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summaryId": {
                    "type": "string",
                    "description": "The summary node ID to expand."
                },
                "maxMessages": {
                    "type": "integer",
                    "description": "Maximum number of messages to return (default 50). Prevents context flooding.",
                    "default": 50
                }
            },
            "required": ["summaryId"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: LcmExpandArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for lcm_expand: {e}"))?;

        // ── Sub-agent restriction check ──
        // lcm_expand is only allowed in sub-agents. The main agent must
        // delegate expansion work via the Task tool to prevent uncontrolled
        // context growth in the primary interaction loop.
        //
        // We check this by looking at the hop_index: the root agent starts
        // at hop 0. Sub-agents will have different context markers.
        // For now, we use a simple heuristic: if the context has a non-zero
        // hop_index or a sub-agent marker, it's allowed.
        //
        // In the full implementation, this check should use the actual
        // sub-agent context from the Task tool infrastructure.
        if !Self::is_sub_agent_context(context) {
            return Err(AgentJaxError::tool(
                "lcm_expand is restricted to ephemeral sub-agents only. \
                 The main agent should delegate expansion work via:\n\n\
                 sub_agent(action='spawn', prompt=\"Expand summary X and report findings\", \
                 subagentType='explore', delegatedScope=[...], keptWork=[...])\n\n\
                 The spawned sub-agent will execute, report results, and be disposed. \
                 This restriction prevents uncontrolled context growth in \
                 the primary conversation loop. See LCM §2.4 for details.",
            ));
        }

        let summary_id = LcmId::from(args.summary_id);

        let store = &self.store;
        let messages = store
            .expand_summary(&summary_id)
            .map_err(|e| format!("lcm_expand failed: {e}"))?;

        // Truncate to max_messages.
        let total_count = messages.len();
        let truncated: Vec<_> = messages
            .into_iter()
            .take(args.max_messages)
            .map(|msg| {
                json!({
                    "id": msg.id.as_str(),
                    "role": msg.role.as_str(),
                    "content": msg.content,
                    "tokenCount": msg.token_count,
                    "timestampUnixMs": msg.timestamp_unix_ms,
                })
            })
            .collect();

        let result = json!({
            "expandedFrom": summary_id.as_str(),
            "totalMessages": total_count,
            "returnedMessages": truncated.len(),
            "truncated": total_count > args.max_messages,
            "messages": truncated,
        });

        Ok(result)
    }
}

impl LcmExpandTool {
    /// Check whether the current execution context is a sub-agent.
    ///
    /// Short-lived sub-agents (spawned via `sub_agent` tool) always have
    /// `sub_agent_id` set by `AgentRuntime::run_turn` when the
    /// conversation_id matches the `/sub-agent/` pattern.
    ///
    /// This is the *only* reliable indicator — `hop_index` and `turn_id`
    /// are not deterministic sub-agent markers and are not checked here.
    fn is_sub_agent_context(context: &ToolExecutionContext) -> bool {
        context.sub_agent_id.is_some()
    }
}
