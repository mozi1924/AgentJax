use crate::message_phase::AssistantPhase;
use crate::provider_api::capabilities::ProviderCapabilities;
use crate::tools::ToolPresentation;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelDescriptor {
    pub id: String,
    #[serde(default, alias = "supported_reasoning_levels")]
    pub supported_reasoning_levels: Vec<String>,
    #[serde(default)]
    pub kind: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ProviderModelMetadata {
    pub context_window: Option<usize>,
    #[serde(default)]
    pub kind: Option<String>,
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
    /// Incremental reasoning / thinking content delta.
    /// Emitted for APIs that stream reasoning tokens separately
    /// (e.g. DeepSeek-R1 `delta.reasoning_content`, OpenAI o-series).
    ReasoningDelta {
        delta: String,
    },
    /// Reasoning phase complete. Carries optional token count for the
    /// thinking block when the provider reports it.
    ReasoningCompleted {
        total_tokens: Option<usize>,
    },
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

    // ── Sampling parameters (Chat Completions native; Responses ignores unsupported) ──
    #[serde(default)]
    pub temperature: Option<f32>,
    #[serde(default)]
    pub top_p: Option<f32>,
    #[serde(default)]
    pub presence_penalty: Option<f32>,
    #[serde(default)]
    pub frequency_penalty: Option<f32>,
    #[serde(default)]
    pub max_tokens: Option<u32>,
    #[serde(default, alias = "max_completion_tokens")]
    pub max_completion_tokens: Option<u32>,
    #[serde(default)]
    pub reasoning_budget_tokens: Option<u32>,

    /// Provider-specific extra body fields to pass through in the request.
    /// These are merged into the request body after standard parameters.
    /// Useful for provider-specific features like DeepSeek's `thinking` field.
    #[serde(default)]
    pub extra_body: BTreeMap<String, Value>,
}

pub type ResponseStreamRequest = ProviderTurnRequest;
pub type ProviderEventSink<'a> =
    dyn FnMut(ProviderStreamEvent) -> crate::error::AgentJaxResult<()> + Send + 'a;

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
#[allow(dead_code)]
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

// ── Embedding Types ──────────────────────────────────────────────────────────

/// A single embedding vector.
#[allow(dead_code)]
pub type Embedding = Vec<f32>;

/// Request to embed one or more text inputs.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct EmbeddingRequest {
    /// Text strings to embed. The provider may restrict batch size.
    pub input: Vec<String>,
    /// Optional model override. When `None`, the provider's default is used.
    pub model: Option<String>,
    /// Dimensions to truncate to. When `None`, the provider's native dimension is used.
    pub dimensions: Option<usize>,
}

#[allow(dead_code)]
impl EmbeddingRequest {
    /// Create a request for a single text string.
    pub fn single(text: impl Into<String>) -> Self {
        Self {
            input: vec![text.into()],
            model: None,
            dimensions: None,
        }
    }

    /// Create a request from a batch of texts.
    pub fn batch(texts: Vec<String>) -> Self {
        Self {
            input: texts,
            model: None,
            dimensions: None,
        }
    }
}

/// Usage statistics for an embedding request.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct EmbeddingUsage {
    /// Tokens consumed by the input prompt.
    pub prompt_tokens: Option<u32>,
    /// Total tokens consumed.
    pub total_tokens: Option<u32>,
}

/// The result of an embedding request.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[allow(dead_code)]
pub struct EmbeddingResponse {
    /// The embedding vectors, one per input text in the same order.
    pub embeddings: Vec<Embedding>,
    /// The model that produced the embeddings.
    pub model: String,
    /// Usage statistics, if available from the provider.
    #[serde(default)]
    pub usage: EmbeddingUsage,
}
