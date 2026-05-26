mod context;
mod file_io;
mod items;
mod mutations;
mod paths;
mod queries;
mod recovery;
mod types;

#[cfg(test)]
mod tests;

const MAX_CONTEXT_ITEMS_PER_REQUEST: usize = 200;

use std::path::PathBuf;

pub use context::load_context_for_request;
#[allow(unused_imports)]
pub use items::{build_assistant_output_items, build_user_input_items, new_conversation_id};
pub use mutations::{
    append_context_item, append_message, delete_conversation, ensure_conversation,
    rename_conversation, update_auto_title,
};
#[allow(unused_imports)]
pub use paths::{conversation_dir_path, conversation_workspace_path};
pub use queries::{list_conversations, load_conversation, load_title_generation_candidate};
pub use recovery::build_recovery_developer_note;
#[allow(unused_imports)]
pub use types::{
    AppendContextItemInput, AppendMessageInput, ConversationContext, ConversationDetail,
    ConversationMessage, ConversationMetaLine, ConversationSummary, TitleGenerationCandidate,
};

#[allow(dead_code)]
pub fn conversations_dir_path() -> Result<PathBuf, String> {
    paths::conversations_dir_path()
}

#[allow(dead_code)]
pub fn ensure_conversations_dir() -> Result<PathBuf, String> {
    paths::ensure_conversations_dir()
}
