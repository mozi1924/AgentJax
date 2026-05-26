use super::context::sanitize_tool_call_pairs;
use super::file_io::{
    read_conversation_file, refresh_meta_derived_fields, summary_from_meta, write_conversation_file,
};
use super::items::{build_assistant_output_items, build_user_input_items};
use super::paths::{
    conversation_dir_path, conversation_messages_path, conversation_metadata_path, ensure_session_layout,
};
use super::types::{
    AppendContextItemInput, AppendMessageInput, ConversationEntryLine, ConversationFileData,
    ConversationMetaLine, ConversationSummary, DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
};
use crate::conversation_store_utils::{normalize_title, now_unix_ms, sanitize_optional};
use std::collections::BTreeMap;
use std::fs;

pub fn ensure_conversation(
    conversation_id: &str,
    utility_model: &str,
) -> Result<ConversationMetaLine, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    if let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? {
        let mut changed = false;

        if data.meta.title.trim().is_empty() {
            data.meta.title = DEFAULT_CONVERSATION_TITLE.to_string();
            changed = true;
        }
        if data.meta.title_source.trim().is_empty() {
            data.meta.title_source = "pending".to_string();
            changed = true;
        }
        if data.meta.utility_model.trim().is_empty() && !utility_model.trim().is_empty() {
            data.meta.utility_model = utility_model.trim().to_string();
            changed = true;
        }

        refresh_meta_derived_fields(&mut data.meta, &data.entries);
        if changed {
            write_conversation_file(&metadata_path, &messages_path, &data)?;
        }
        return Ok(data.meta);
    }

    let now = now_unix_ms();
    let data = ConversationFileData {
        meta: ConversationMetaLine {
            version: LOG_VERSION,
            record_type: "meta".to_string(),
            conversation_id: conversation_id.to_string(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            title: DEFAULT_CONVERSATION_TITLE.to_string(),
            title_source: "pending".to_string(),
            utility_model: utility_model.trim().to_string(),
            message_count: 0,
            last_message_at_unix_ms: 0,
            last_message_preview: String::new(),
            metadata: BTreeMap::new(),
        },
        entries: Vec::new(),
    };

    ensure_session_layout(conversation_id)?;
    write_conversation_file(&metadata_path, &messages_path, &data)?;
    Ok(data.meta)
}

pub fn append_message(input: AppendMessageInput, utility_model: &str) -> Result<(), String> {
    let role = input.role.trim().to_string();
    let text = input.text.trim().to_string();
    let context_items = if !input.context_items.is_empty() {
        input.context_items
    } else if role == "user" {
        build_user_input_items(&text)
    } else if role == "assistant" {
        build_assistant_output_items(&text)
    } else {
        Vec::new()
    };
    let context_items = sanitize_tool_call_pairs(context_items);

    append_entry_line(
        &input.conversation_id,
        utility_model,
        ConversationEntryLine {
            version: LOG_VERSION,
            record_type: "message".to_string(),
            entry_id: input.entry_id,
            created_at_unix_ms: input.created_at_unix_ms,
            role: Some(role),
            text: Some(text),
            response_id: sanitize_optional(input.response_id),
            provider: sanitize_optional(input.provider),
            model_profile: sanitize_optional(input.model_profile),
            model_id: sanitize_optional(input.model_id),
            request_id: sanitize_optional(input.request_id),
            context_items,
            tool_name: None,
            tool_call_id: None,
            tool_arguments: None,
            tool_output: None,
            timeline_events: input.timeline_events,
            metadata: input.metadata,
        },
    )
}

pub fn append_context_item(input: AppendContextItemInput, utility_model: &str) -> Result<(), String> {
    append_entry_line(
        &input.conversation_id,
        utility_model,
        ConversationEntryLine {
            version: LOG_VERSION,
            record_type: "context_item".to_string(),
            entry_id: input.entry_id,
            created_at_unix_ms: input.created_at_unix_ms,
            role: None,
            text: None,
            response_id: sanitize_optional(input.response_id),
            provider: sanitize_optional(input.provider),
            model_profile: sanitize_optional(input.model_profile),
            model_id: sanitize_optional(input.model_id),
            request_id: sanitize_optional(input.request_id),
            context_items: vec![input.context_item],
            tool_name: None,
            tool_call_id: None,
            tool_arguments: None,
            tool_output: None,
            timeline_events: None,
            metadata: input.metadata,
        },
    )
}

fn append_entry_line(
    conversation_id: &str,
    utility_model: &str,
    entry: ConversationEntryLine,
) -> Result<(), String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let mut data = if let Some(existing) = read_conversation_file(&metadata_path, &messages_path)? {
        existing
    } else {
        ConversationFileData {
            meta: ensure_conversation(conversation_id, utility_model)?,
            entries: Vec::new(),
        }
    };
    if data
        .entries
        .iter()
        .any(|existing| existing.entry_id == entry.entry_id)
    {
        return Ok(());
    }
    data.entries.push(entry);

    refresh_meta_derived_fields(&mut data.meta, &data.entries);
    if data.meta.utility_model.trim().is_empty() && !utility_model.trim().is_empty() {
        data.meta.utility_model = utility_model.trim().to_string();
    }
    write_conversation_file(&metadata_path, &messages_path, &data)
}

pub fn rename_conversation(
    conversation_id: &str,
    title: &str,
    utility_model: &str,
) -> Result<ConversationSummary, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let mut data = if let Some(existing) = read_conversation_file(&metadata_path, &messages_path)? {
        existing
    } else {
        ConversationFileData {
            meta: ensure_conversation(conversation_id, utility_model)?,
            entries: Vec::new(),
        }
    };

    data.meta.title = normalize_title(title);
    data.meta.title_source = "manual".to_string();
    data.meta.updated_at_unix_ms = now_unix_ms();
    refresh_meta_derived_fields(&mut data.meta, &data.entries);
    write_conversation_file(&metadata_path, &messages_path, &data)?;
    Ok(summary_from_meta(&data.meta))
}

pub fn update_auto_title(
    conversation_id: &str,
    title: &str,
) -> Result<Option<ConversationSummary>, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };

    if data.meta.title_source == "manual" {
        return Ok(Some(summary_from_meta(&data.meta)));
    }

    data.meta.title = normalize_title(title);
    data.meta.title_source = "auto".to_string();
    data.meta.updated_at_unix_ms = now_unix_ms();
    refresh_meta_derived_fields(&mut data.meta, &data.entries);
    write_conversation_file(&metadata_path, &messages_path, &data)?;
    Ok(Some(summary_from_meta(&data.meta)))
}

pub fn delete_conversation(conversation_id: &str) -> Result<bool, String> {
    let dir = conversation_dir_path(conversation_id)?;
    if !dir.exists() {
        return Ok(false);
    }

    fs::remove_dir_all(&dir)
        .map_err(|e| format!("Failed to delete session dir {}: {e}", dir.display()))?;
    Ok(true)
}

