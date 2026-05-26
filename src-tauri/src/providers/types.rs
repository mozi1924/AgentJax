use super::capabilities::ProviderCapabilities;
use crate::message_phase::AssistantPhase;
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", content = "data")]
pub enum ProviderStreamEvent {
    ReasoningStarted,
    OutputTextStarted,
    OutputTextDelta {
        delta: String,
        phase: Option<AssistantPhase>,
    },
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
        name: String,
        output: String,
    },
    /// Emitted after each model-response hop completes.
    HopAssistantText {
        text: String,
        phase: AssistantPhase,
        response_id: String,
    },
    ResponseCompleted,
}

#[derive(Debug, Clone, Default)]
pub struct ProviderTurnRequest {
    pub input_items: Vec<Value>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub instructions_override: Option<String>,
    pub text: Option<Value>,
    pub include: Option<Vec<String>>,
    pub service_tier: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub client_metadata: Option<Value>,
    pub generate: Option<bool>,
    pub tools: Option<Vec<Value>>,
    pub tool_choice: Option<Value>,
    /// Hint to the Responses API that this request continues a previous
    /// response.  The API may use this to optimise internal state (e.g.
    /// reuse cached key-value lookups) but the agent **never** relies on
    /// server-side context — `store` is always `false` and the full
    /// accumulated input items are always replayed locally.
    pub previous_response_id: Option<String>,
}

pub type ResponseStreamRequest = ProviderTurnRequest;
pub type ProviderEventSink<'a> = dyn FnMut(ProviderStreamEvent) -> Result<(), String> + Send + 'a;

#[derive(Debug, Clone)]
pub struct ProviderPendingToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

#[derive(Debug, Clone)]
pub struct ResponseStreamResult {
    pub response_id: String,
    /// Final answer text (after all tool calls are complete).
    pub output_text: String,
    pub output_items: Vec<Value>,
    pub provider_key: String,
    pub model_profile: String,
    pub model_id: String,
    pub capabilities: ProviderCapabilities,
}
