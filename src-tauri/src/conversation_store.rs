mod context;
mod file_io;
mod locks;
mod mutations;
mod paths;
mod queries;
mod recovery;
mod types;

#[cfg(test)]
mod tests;

use std::path::PathBuf;

pub use context::load_context_for_request;
pub use mutations::{
    append_line, delete_conversation, ensure_conversation, remove_conversation_dynamic_tool,
    rename_conversation, update_auto_title, update_conversation_dynamic_tools,
    update_conversation_mounted_tool_sources, update_line, upsert_conversation_dynamic_tool,
};
#[cfg(test)]
pub use paths::conversation_dir_path;
pub use paths::conversation_workspace_path;
pub use queries::{
    list_conversations, load_conversation, load_conversation_dynamic_tools,
    load_conversation_mounted_tool_sources, load_title_generation_candidate,
};
pub use recovery::build_recovery_developer_note;
pub use types::{
    AppendLineInput, AssistantLine, AssistantStatus, ConversationDetail, ConversationDynamicTool,
    ConversationDynamicToolBinding, ConversationLine, ConversationMountedToolSource,
    ConversationMountedToolDefinition, ConversationSummary, TitleGenerationCandidate, ToolLine,
    ToolStatus, UpdateLineInput, UserLine,
};

/// Generate a new unique conversation id.
pub fn new_conversation_id() -> String {
    use crate::conversation_store_utils::today_utc_yyyy_mm_dd;
    use uuid::Uuid;
    format!("{}-{}", today_utc_yyyy_mm_dd(), Uuid::new_v4())
}

#[allow(dead_code)]
pub fn conversations_dir_path() -> Result<PathBuf, String> {
    paths::conversations_dir_path()
}

#[allow(dead_code)]
pub fn ensure_conversations_dir() -> Result<PathBuf, String> {
    paths::ensure_conversations_dir()
}
