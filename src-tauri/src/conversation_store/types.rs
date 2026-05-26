use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use crate::message_phase::AssistantPhase;

pub const LOG_VERSION: u32 = 6;
pub const DEFAULT_CONVERSATION_TITLE: &str = "新对话";

// ── Metadata (metadata.json) ──────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMeta {
    pub version: u32,
    pub conversation_id: String,
    pub created_at_unix_ms: i64,
    pub updated_at_unix_ms: i64,
    pub title: String,
    pub title_source: String,
    pub message_count: usize,
    pub last_message_preview: String,
    #[serde(default)]
    pub conversation_type: String,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

// ── Conversation lines (messages.jsonl) ───────────────────────────────────
// Tagged-union: each line is self-describing via `kind`.

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum ConversationLine {
    #[serde(rename = "user")]
    User(UserLine),
    #[serde(rename = "tool")]
    Tool(ToolLine),
    #[serde(rename = "assistant")]
    Assistant(AssistantLine),
}

impl ConversationLine {
    pub fn request_id(&self) -> Option<&str> {
        match self {
            ConversationLine::User(l) => Some(&l.request_id),
            ConversationLine::Tool(l) => Some(&l.request_id),
            ConversationLine::Assistant(l) => Some(&l.request_id),
        }
    }

    pub fn ts(&self) -> i64 {
        match self {
            ConversationLine::User(l) => l.ts,
            ConversationLine::Tool(l) => l.ts,
            ConversationLine::Assistant(l) => l.ts,
        }
    }

    pub fn id(&self) -> &str {
        match self {
            ConversationLine::User(l) => &l.id,
            ConversationLine::Tool(l) => &l.id,
            ConversationLine::Assistant(l) => &l.id,
        }
    }

    /// Does this line represent a "message" for the purpose of
    /// counting and preview generation?
    pub fn is_message(&self) -> bool {
        matches!(self, ConversationLine::User(_) | ConversationLine::Assistant(_))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserLine {
    pub id: String,
    pub ts: i64,
    pub request_id: String,
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolLine {
    pub id: String,
    pub ts: i64,
    pub request_id: String,
    pub call_id: String,
    pub name: String,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output: Option<Value>,
    #[serde(default = "default_tool_status")]
    pub status: ToolStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolStatus {
    Pending,
    Done,
    Failed,
}

fn default_tool_status() -> ToolStatus {
    ToolStatus::Pending
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssistantLine {
    pub id: String,
    pub ts: i64,
    pub request_id: String,
    #[serde(rename = "responseId")]
    pub response_id: String,
    pub phase: AssistantPhase,
    /// The assistant's text for this line.
    pub text: String,
    #[serde(default = "default_assistant_status")]
    pub status: AssistantStatus,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssistantStatus {
    Draft,
    Done,
}

fn default_assistant_status() -> AssistantStatus {
    AssistantStatus::Draft
}

// ── In-memory file representation ─────────────────────────────────────────

#[derive(Debug, Clone)]
pub(crate) struct ConversationData {
    pub meta: ConversationMeta,
    pub lines: Vec<ConversationLine>,
}

// ── Frontend-facing DTOs ──────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub title: String,
    pub title_source: String,
    pub message_count: usize,
    pub last_message_preview: String,
    pub updated_at_unix_ms: i64,
    #[serde(default)]
    pub conversation_type: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetail {
    pub conversation_id: String,
    pub title: String,
    pub title_source: String,
    pub lines: Vec<ConversationLine>,
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

// ── Input types for mutations ─────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct AppendLineInput {
    pub conversation_id: String,
    pub line: ConversationLine,
}

#[derive(Debug, Clone)]
pub struct UpdateLineInput {
    pub conversation_id: String,
    pub line_id: String,
    pub line: ConversationLine,
}
