//! `lcm_grep` — regex search across the immutable message history.
//!
//! Corresponds to Appendix C.1 of the LCM paper.
//!
//! This tool allows the model to search the full conversation history
//! using regex patterns. Results are paginated and grouped by their
//! covering summary node to provide conversational context.

use crate::error::{AgentJaxError, AgentJaxResult};
use crate::lcm::store::LcmStore;
use crate::lcm::types::LcmId;
use crate::tools::{Tool, ToolExecutionContext};
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

pub struct LcmGrepTool {
    store: Option<Arc<LcmStore>>,
}

impl LcmGrepTool {
    pub fn new(store: Arc<LcmStore>) -> Self {
        Self { store: Some(store) }
    }

    /// Create a tool without a backing store (for registration / snapshot only).
    /// The tool will fail at execution time if no store is wired in later.
    pub fn without_store() -> Self {
        Self { store: None }
    }

    fn store(&self) -> AgentJaxResult<&LcmStore> {
        self.store
            .as_deref()
            .ok_or_else(|| AgentJaxError::tool("lcm_grep requires an active conversation (no LCM store available)"))
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LcmGrepArgs {
    /// The regex pattern to search for.
    pattern: String,
    /// Optional summary ID to scope the search.
    summary_id: Option<String>,
    /// Optional pagination cursor.
    cursor: Option<String>,
}

#[async_trait::async_trait]
impl Tool for LcmGrepTool {
    fn name(&self) -> &'static str {
        "lcm_grep"
    }

    fn description(&self) -> &'static str {
        "Search the full conversation history using regex patterns. \
         Returns matching messages with their context, grouped by the \
         summary node that covers them. Results are paginated to prevent \
         context flooding. Use summaryId to restrict the search to a \
         specific region of the conversation."
    }

    fn display_name(&self) -> &'static str {
        "LCM Grep"
    }

    fn icon(&self) -> Option<&'static str> {
        Some("search")
    }

    fn parameters_schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regex pattern to search for in the conversation history."
                },
                "summaryId": {
                    "type": "string",
                    "description": "Optional: restrict search to messages covered by this summary node."
                },
                "cursor": {
                    "type": "string",
                    "description": "Optional: pagination cursor from a previous search result."
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(
        &self,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> AgentJaxResult<Value> {
        let args: LcmGrepArgs = serde_json::from_value(arguments.clone())
            .map_err(|e| format!("Invalid arguments for lcm_grep: {e}"))?;

        let conversation_id = context
            .conversation_id
            .as_deref()
            .ok_or_else(|| "lcm_grep requires a conversation_id".to_string())?;

        let summary_id = args.summary_id.map(LcmId::from);

        let store = self.store()?;
        let page_size = store.grep_page_size();

        let results = store
            .search_messages(
                conversation_id,
                &args.pattern,
                summary_id.as_ref(),
                args.cursor.as_deref(),
                page_size,
            )
            .map_err(|e| AgentJaxError::internal(format!("lcm_grep search failed: {e}")))?;

        serde_json::to_value(&results)
            .map_err(|e| AgentJaxError::internal(format!("Failed to serialize grep results: {e}")))
    }
}
