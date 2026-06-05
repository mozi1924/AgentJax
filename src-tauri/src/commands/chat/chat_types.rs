use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub input: String,
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    /// Reasoning configuration. The frontend sends this as `reasoning`
    /// (an object with `enabled`, `effort`, `budgetTokens`).
    /// Replaces the old `reasoningEffort` string field.
    #[serde(default)]
    pub reasoning: Option<crate::provider_api::types::ReasoningConfig>,
    pub text: Option<Value>,
    pub include: Option<Vec<String>>,
    pub service_tier: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub client_metadata: Option<Value>,
    pub generate: Option<bool>,
    pub request_id: Option<String>,

    /// Agent profile to use for this request. Defaults to "main".
    #[serde(default)]
    pub agent_id: Option<String>,

    // ── Sampling parameters ──
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelChatRequest {
    pub request_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadConversationRequest {
    pub conversation_id: String,
    pub model: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameConversationRequest {
    pub conversation_id: String,
    pub title: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConversationRequest {
    pub conversation_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadConversationDynamicToolsRequest {
    pub conversation_id: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceConversationDynamicToolsRequest {
    pub conversation_id: String,
    pub tools: Vec<crate::conversation_store::ConversationDynamicTool>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertConversationDynamicToolRequest {
    pub conversation_id: String,
    pub tool: crate::conversation_store::ConversationDynamicTool,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveConversationDynamicToolRequest {
    pub conversation_id: String,
    pub tool_name: String,
    #[serde(default)]
    pub agent_id: Option<String>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatResponse {
    pub response_id: String,
    pub output_text: String,
    pub conversation_id: String,
    pub conversation_title: Option<String>,
    pub context_token_count: usize,
}
