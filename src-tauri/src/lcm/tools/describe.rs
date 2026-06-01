//! `lcm_describe` — metadata retrieval for LCM entities.
//!
//! Corresponds to Appendix C.1 of the LCM paper.
//!
//! Returns structured metadata for any LCM entity:
//! - Messages: role, token count, timestamp, whether covered by a summary
//! - Summaries: kind, compaction level, children, file references, full text
//! - File references: path, MIME type, token count, exploration summary

use crate::lcm::store::LcmStore;
use crate::lcm::types::LcmId;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct LcmDescribeTool {
    store: Arc<LcmStore>,
}

impl LcmDescribeTool {
    pub fn new(store: Arc<LcmStore>) -> Self {
        Self { store }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LcmDescribeArgs {
    /// The LCM entity ID to describe.
    id: String,
}

impl Tool for LcmDescribeTool {
    fn name(&self) -> &'static str {
        "lcm_describe"
    }

    fn description(&self) -> &'static str {
        "Get metadata for any LCM entity (message, summary node, or file reference). \
         For messages: returns role, token count, timestamp, and whether covered \
         by a summary. For summaries: returns kind, compaction level, child count, \
         file references, and full text. For files: returns path, MIME type, \
         token count, and exploration summary."
    }

    fn display_name(&self) -> &'static str {
        "LCM Describe"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("info")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "The LCM entity ID to describe (message ID, summary ID, or file reference ID)."
                }
            },
            "required": ["id"]
        })
    }

    fn execute(
        &self,
        arguments: &Value,
        _context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: LcmDescribeArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for lcm_describe: {e}"))?;

        let id = LcmId::from(args.id);

        let result = self
            .store
            .describe(&id)
            .map_err(|e| AgentJaxError::internal(format!("lcm_describe failed: {e}")))?;

        match result {
            Some(desc) => {
                serde_json::to_value(&desc)
                    .map_err(|e| AgentJaxError::internal(format!("Failed to serialize describe result: {e}")))
            }
            None => Err(AgentJaxError::not_found(format!("No LCM entity found with id: {id}"))),
        }
    }
}
