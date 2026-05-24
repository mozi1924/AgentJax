use serde::{Deserialize, Serialize};
use serde_json::Value;
use super::capabilities::ProviderCapabilities;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelDescriptor {
    pub id: String,
    #[serde(default, alias = "supported_reasoning_levels")]
    pub supported_reasoning_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningCapability {
    pub supports_reasoning: bool,
    pub supported_reasoning_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ProviderStreamEvent {
    ReasoningStarted,
    OutputTextStarted,
    OutputTextDelta(String),
    ToolCallStarted {
        item_id: String,
        call_id: String,
        name: String,
    },
    ToolCallArgumentsDelta {
        item_id: String,
        call_id: String,
        delta: String,
    },
    ToolCallCompleted {
        item_id: String,
        call_id: String,
        name: String,
        arguments: String,
    },
    ToolCallExecuted {
        call_id: String,
        output: String,
    },
    ResponseCompleted,
}

#[derive(Debug, Clone, Default)]
pub struct ResponseStreamRequest {
    pub input_text: String,
    pub previous_response_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub context_items: Vec<Value>,
    pub instructions_override: Option<String>,
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct ResponseStreamResult {
    pub response_id: String,
    pub output_text: String,
    pub output_items: Vec<Value>,
    pub provider_key: String,
    pub model_profile: String,
    pub model_id: String,
    pub capabilities: ProviderCapabilities,
}
