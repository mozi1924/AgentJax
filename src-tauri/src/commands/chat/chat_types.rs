use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatRequest {
    pub input: String,
    pub conversation_id: Option<String>,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub text: Option<Value>,
    pub include: Option<Vec<String>>,
    pub service_tier: Option<String>,
    pub prompt_cache_key: Option<String>,
    pub client_metadata: Option<Value>,
    pub generate: Option<bool>,
    pub request_id: Option<String>,
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
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameConversationRequest {
    pub conversation_id: String,
    pub title: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteConversationRequest {
    pub conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LoadConversationDynamicToolsRequest {
    pub conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ReplaceConversationDynamicToolsRequest {
    pub conversation_id: String,
    pub tools: Vec<crate::conversation_store::ConversationDynamicTool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpsertConversationDynamicToolRequest {
    pub conversation_id: String,
    pub tool: crate::conversation_store::ConversationDynamicTool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoveConversationDynamicToolRequest {
    pub conversation_id: String,
    pub tool_name: String,
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
