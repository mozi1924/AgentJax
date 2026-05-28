use crate::message_phase::AssistantPhase;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub const LOG_VERSION: u32 = 6;
pub const DEFAULT_CONVERSATION_TITLE: &str = "新对话";
pub const CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY: &str = "dynamic_tools";
pub const CONVERSATION_MOUNTED_MCP_SERVERS_METADATA_KEY: &str = "mounted_mcp_servers";
pub const CONVERSATION_MOUNTED_TOOL_SOURCES_METADATA_KEY: &str = "mounted_tool_sources";

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

/// Conversation-scoped dynamic tool definition persisted in metadata.json.
///
/// These tools are model-visible aliases whose execution is routed to a stable
/// local binding (native tool or MCP tool). Persisting them at the conversation
/// layer lets future turns reconstruct the same logical tool set without
/// depending on ad-hoc in-memory registration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDynamicTool {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub parameters: Value,
    pub binding: ConversationDynamicToolBinding,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ConversationDynamicToolBinding {
    Native { tool: String },
    Mcp { server_id: String, tool: String },
}

/// Conversation-scoped MCP mount state persisted in metadata.json.
///
/// Each entry captures the logical tool surface exposed by a mounted MCP
/// server so future turns, reloads, and app restarts can restore the same
/// compact-yet-usable tool view without forcing the agent to remount first.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMountedMcpServer {
    pub server_id: String,
    pub tools: Vec<ConversationMountedMcpToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMountedMcpToolDefinition {
    pub tool_name: String,
    #[serde(default)]
    pub display_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub input_schema: Value,
}

/// Conversation-scoped generic tool source mount state persisted in metadata.json.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMountedToolSource {
    pub source_id: String,
    pub source_type: String, // e.g. "mcp" or "plugin"
    pub tools: Vec<ConversationMountedToolDefinition>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ConversationMountedToolDefinition {
    pub tool_name: String,
    #[serde(default)]
    pub display_name: String,
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub input_schema: Value,
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

    /// Whether this line should contribute to sidebar-style conversation
    /// summaries such as message counts and preview text.
    ///
    /// Commentary lines remain persisted and replayed into model context, but
    /// they are intentionally excluded from user-facing summary metadata so
    /// the sidebar tracks substantive user/final-answer exchanges rather than
    /// in-progress narration.
    pub fn contributes_to_summary(&self) -> bool {
        match self {
            ConversationLine::User(line) => !line.text.trim().is_empty(),
            ConversationLine::Assistant(line) => line.is_visible_summary_message(),
            ConversationLine::Tool(_) => false,
        }
    }

    /// Returns the text that should be used for summary previews when this
    /// line contributes to user-facing metadata.
    pub fn summary_preview_text(&self) -> Option<&str> {
        match self {
            ConversationLine::User(line) => {
                let text = line.text.trim();
                (!text.is_empty()).then_some(text)
            }
            ConversationLine::Assistant(line) => line.summary_preview_text(),
            ConversationLine::Tool(_) => None,
        }
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
    #[serde(default)]
    pub started_ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_ts: Option<i64>,
    pub request_id: String,
    pub call_id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    pub args: Value,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub output: Option<Value>,
    #[serde(default = "default_tool_status")]
    pub status: ToolStatus,
}

impl ToolLine {
    pub fn started_at_unix_ms(&self) -> i64 {
        if self.started_ts > 0 {
            self.started_ts
        } else {
            self.ts
        }
    }

    pub fn completed_at_unix_ms(&self) -> Option<i64> {
        self.completed_ts.or_else(|| match self.status {
            ToolStatus::Done | ToolStatus::Failed => Some(self.ts),
            ToolStatus::Pending => None,
        })
    }
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<AssistantPhase>,
    /// The assistant's text for this line.
    pub text: String,
    #[serde(default = "default_assistant_status")]
    pub status: AssistantStatus,
}

impl AssistantLine {
    pub fn is_final_or_unknown(&self) -> bool {
        self.phase != Some(AssistantPhase::Commentary)
    }

    /// Assistant lines count toward user-facing conversation summaries only
    /// when they represent a completed non-commentary message with visible
    /// text.
    pub fn is_visible_summary_message(&self) -> bool {
        self.status == AssistantStatus::Done
            && self.is_final_or_unknown()
            && !self.text.trim().is_empty()
    }

    pub fn summary_preview_text(&self) -> Option<&str> {
        let text = self.text.trim();
        (self.is_visible_summary_message() && !text.is_empty()).then_some(text)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
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
