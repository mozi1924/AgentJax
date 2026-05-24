use serde::{Deserialize, Serialize};
use serde_json::Value;

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

#[derive(Debug, Clone)]
pub enum ProviderStreamEvent {
    ReasoningStarted,
    OutputTextStarted,
    OutputTextDelta(String),
}

#[derive(Debug, Clone, Default)]
pub struct ResponseStreamRequest {
    pub input_text: String,
    pub previous_response_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub context_items: Vec<Value>,
    pub instructions_override: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ResponseStreamResult {
    pub response_id: String,
    pub output_text: String,
    pub output_items: Vec<Value>,
    pub provider_key: String,
    pub model_profile: String,
    pub model_id: String,
}
