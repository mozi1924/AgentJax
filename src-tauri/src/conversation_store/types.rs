use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const LOG_VERSION: u32 = 3;
pub const DEFAULT_CONVERSATION_TITLE: &str = "新对话";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMetaLine {
    pub version: u32,
    pub record_type: String,
    pub conversation_id: String,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub title: String,
    pub title_source: String,
    pub utility_model: String,
    pub message_count: usize,
    pub last_message_at_unix_ms: i64,
    pub last_message_preview: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationEntryLine {
    pub version: u32,
    pub record_type: String,
    pub entry_id: String,
    pub created_at_unix_ms: i64,
    pub role: Option<String>,
    pub text: Option<String>,
    pub response_id: Option<String>,
    pub provider: Option<String>,
    pub model_profile: Option<String>,
    pub model_id: Option<String>,
    pub request_id: Option<String>,
    #[serde(default)]
    pub context_items: Vec<Value>,
    pub tool_name: Option<String>,
    pub tool_call_id: Option<String>,
    pub tool_arguments: Option<Value>,
    pub tool_output: Option<Value>,
    #[serde(default)]
    pub timeline_events: Option<Vec<Value>>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct AppendMessageInput {
    pub conversation_id: String,
    pub entry_id: String,
    pub role: String,
    pub text: String,
    pub created_at_unix_ms: i64,
    pub response_id: Option<String>,
    pub provider: Option<String>,
    pub model_profile: Option<String>,
    pub model_id: Option<String>,
    pub request_id: Option<String>,
    pub context_items: Vec<Value>,
    pub timeline_events: Option<Vec<Value>>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub title: String,
    pub title_source: String,
    pub message_count: usize,
    pub last_message_preview: String,
    pub last_message_at_unix_ms: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMessage {
    pub id: String,
    pub role: String,
    pub text: String,
    pub created_at_unix_ms: i64,
    pub response_id: Option<String>,
    #[serde(default)]
    pub context_items: Vec<Value>,
    #[serde(default)]
    pub timeline_events: Option<Vec<Value>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetail {
    pub conversation_id: String,
    pub title: String,
    pub title_source: String,
    pub last_response_id: Option<String>,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    pub input_items: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct TitleGenerationCandidate {
    pub user_text: String,
    pub assistant_text: String,
}

#[derive(Debug, Clone)]
pub(crate) struct ConversationFileData {
    pub meta: ConversationMetaLine,
    pub entries: Vec<ConversationEntryLine>,
}
