use crate::provider_api::capabilities::ProviderCapabilities;
use crate::message_phase::AssistantPhase;
use crate::tools::ToolPresentation;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelDescriptor {
    pub id: String,
    #[serde(default, alias = "supported_reasoning_levels")]
    pub supported_reasoning_levels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelMetadata {
    pub context_window: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelReasoningCapability {
    pub supports_reasoning: bool,
    pub supported_reasoning_levels: Vec<String>,
}

/// Internal normalized stream events consumed by the runtime and UI.
///
/// Provider adapters should translate their upstream protocol into this shape
/// before events reach `runtime::engine`. For APIs that do not expose Responses
/// style IDs (for example Gemini function calls), adapters must synthesize
/// stable IDs for the current response hop so tool execution, persistence, and
/// token usage metadata all refer to the same logical objects.
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
        presentation: Option<ToolPresentation>,
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
        presentation: Option<ToolPresentation>,
    },
    ToolCallProgress {
        call_id: String,
        name: String,
        elapsed_ms: u64,
        presentation: Option<ToolPresentation>,
    },
    ToolCallExecuted {
        call_id: String,
        name: String,
        output: String,
        is_success: bool,
        started_at_unix_ms: i64,
        completed_at_unix_ms: i64,
        duration_ms: u64,
        presentation: Option<ToolPresentation>,
    },
    /// Emitted when the provider finishes an assistant message item.
    /// This preserves the provider's original item ordering for persistence.
    AssistantMessageCompleted {
        text: String,
        phase: Option<AssistantPhase>,
        response_id: String,
    },
    /// Emitted after each model-response hop completes.
    HopAssistantText {
        text: String,
        phase: Option<AssistantPhase>,
        response_id: String,
    },
    UsageUpdated {
        response_id: String,
        usage: ProviderUsage,
        aggregate_usage: ProviderUsage,
    },
    ResponseCompleted,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
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
}

pub type ResponseStreamRequest = ProviderTurnRequest;
pub type ProviderEventSink<'a> = dyn FnMut(ProviderStreamEvent) -> crate::error::AgentJaxResult<()> + Send + 'a;

/// Provider-reported token usage in AgentJax's canonical field names.
///
/// This intentionally accepts aliases used by common APIs and gateways. Runtime
/// code treats this as authoritative billing data; local tokenizers are only a
/// fallback when an adapter cannot obtain upstream usage.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsage {
    #[serde(
        default,
        alias = "prompt_tokens",
        alias = "input_tokens",
        alias = "inputTokens",
        alias = "input_token_count",
        alias = "promptTokenCount"
    )]
    pub prompt_tokens: usize,
    #[serde(
        default,
        alias = "completion_tokens",
        alias = "output_tokens",
        alias = "outputTokens",
        alias = "output_token_count",
        alias = "candidatesTokenCount"
    )]
    pub completion_tokens: usize,
    #[serde(
        default,
        alias = "total_tokens",
        alias = "totalTokens",
        alias = "total_token_count",
        alias = "totalTokenCount"
    )]
    pub total_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderUsageRecord {
    pub response_id: String,
    pub usage: ProviderUsage,
}

impl ProviderUsage {
    #[allow(dead_code)] // Reserved API
    pub fn from_api_value(value: &Value) -> Option<Self> {
        let usage_value = value
            .get("response")
            .and_then(|response| response.get("usage"))
            .or_else(|| value.get("usage"))
            .unwrap_or(value);

        let mut usage = serde_json::from_value::<ProviderUsage>(usage_value.clone()).ok()?;
        if usage.total_tokens == 0 {
            usage.total_tokens = usage.prompt_tokens.saturating_add(usage.completion_tokens);
        }

        (usage.prompt_tokens > 0 || usage.completion_tokens > 0 || usage.total_tokens > 0)
            .then_some(usage)
    }

    pub fn saturating_add(&mut self, other: &ProviderUsage) {
        self.prompt_tokens = self.prompt_tokens.saturating_add(other.prompt_tokens);
        self.completion_tokens = self
            .completion_tokens
            .saturating_add(other.completion_tokens);
        self.total_tokens = self.total_tokens.saturating_add(other.total_tokens);
    }
}

#[derive(Debug, Clone)]
pub struct ProviderPendingToolCall {
    pub call_id: String,
    pub name: String,
    pub arguments: Value,
}

/// Complete result for one provider response hop.
///
/// The fields are already normalized for AgentJax. Raw upstream payloads should
/// be converted inside the provider adapter rather than leaked into runtime
/// logic, keeping future native Gemini/Anthropic/Chat Completions adapters
/// independent from the OpenAI Responses event grammar.
#[derive(Debug, Clone)]
#[allow(dead_code)] // Reserved for future use — fields consumed by pending integration
pub struct ResponseStreamResult {
    pub response_id: String,
    /// Final answer text (after all tool calls are complete).
    pub output_text: String,
    pub output_items: Vec<Value>,
    /// Provider-reported billing usage, preferred over local token estimates.
    pub usage: Option<ProviderUsage>,
    pub usage_hops: Vec<ProviderUsageRecord>,
    pub provider_key: String,
    pub model_profile: String,
    pub model_id: String,
    pub capabilities: ProviderCapabilities,
}
