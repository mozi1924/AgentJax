mod file_io;
mod paths;
mod types;

use crate::conversation_store::file_io::{
    normalized_meta_title, read_conversation_file, refresh_meta_derived_fields, summary_from_meta,
    write_conversation_file,
};
use crate::conversation_store::paths::{
    conversation_messages_path, conversation_metadata_path, ensure_session_layout,
    list_conversation_ids,
};
use crate::conversation_store::types::{ConversationEntryLine, ConversationFileData, DEFAULT_CONVERSATION_TITLE, LOG_VERSION};
use crate::conversation_store_utils::{
    normalize_title, normalize_title_source, now_unix_ms, sanitize_optional, today_utc_yyyy_mm_dd,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

pub use paths::agentjax_home_dir;

pub fn new_conversation_id() -> String {
    format!("{}-{}", today_utc_yyyy_mm_dd(), Uuid::new_v4())
}

pub fn build_user_input_items(text: &str) -> Vec<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    vec![json!({
        "role": "user",
        "content": [{
            "type": "input_text",
            "text": trimmed
        }]
    })]
}

pub fn build_assistant_output_items(text: &str) -> Vec<Value> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    vec![json!({
        "type": "message",
        "role": "assistant",
        "status": "completed",
        "content": [{
            "type": "output_text",
            "text": trimmed,
            "annotations": []
        }]
    })]
}

pub use paths::{conversation_dir_path, conversation_workspace_path};
pub use types::{
    AppendMessageInput, ConversationContext, ConversationDetail, ConversationMessage, ConversationMetaLine,
    ConversationSummary, TitleGenerationCandidate,
};

#[allow(dead_code)]
pub fn conversations_dir_path() -> Result<PathBuf, String> {
    paths::conversations_dir_path()
}

#[allow(dead_code)]
pub fn ensure_conversations_dir() -> Result<PathBuf, String> {
    paths::ensure_conversations_dir()
}

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
    let metadata_path = conversation_metadata_path(&input.conversation_id)?;
    let messages_path = conversation_messages_path(&input.conversation_id)?;
    let mut data = if let Some(existing) = read_conversation_file(&metadata_path, &messages_path)? {
        existing
    } else {
        ConversationFileData {
            meta: ensure_conversation(&input.conversation_id, utility_model)?,
            entries: Vec::new(),
        }
    };

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

    data.entries.push(ConversationEntryLine {
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
    });

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

pub fn load_context_for_request(conversation_id: &str) -> Result<ConversationContext, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(ConversationContext::default());
    };

    data.entries.sort_by_key(|entry| entry.created_at_unix_ms);

    let mut context = ConversationContext::default();
    for entry in data.entries {
        if entry.record_type != "message" {
            if !entry.context_items.is_empty() {
                context.input_items.extend(entry.context_items);
            }
            continue;
        }

        if !entry.context_items.is_empty() {
            context.input_items.extend(entry.context_items);
            continue;
        }

        match entry.role.as_deref() {
            Some("user") => {
                if let Some(text) = entry.text.as_deref() {
                    context.input_items.extend(build_user_input_items(text));
                }
            }
            Some("assistant") => {
                if let Some(text) = entry.text.as_deref() {
                    context
                        .input_items
                        .extend(build_assistant_output_items(text));
                }
            }
            _ => {}
        }
    }

    Ok(context)
}

pub fn list_conversations() -> Result<Vec<ConversationSummary>, String> {
    let mut out = Vec::new();

    for conversation_id in list_conversation_ids()? {
        let metadata_path = conversation_metadata_path(&conversation_id)?;
        let messages_path = conversation_messages_path(&conversation_id)?;
        let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
            continue;
        };
        out.push(summary_from_meta(&data.meta));
    }

    out.sort_by(|a, b| {
        b.last_message_at_unix_ms
            .cmp(&a.last_message_at_unix_ms)
            .then_with(|| b.conversation_id.cmp(&a.conversation_id))
    });
    Ok(out)
}

pub fn load_conversation(conversation_id: &str) -> Result<Option<ConversationDetail>, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };

    data.entries.sort_by_key(|entry| entry.created_at_unix_ms);

    let mut last_response_id = None;
    let mut messages = Vec::new();

    for entry in data.entries {
        if entry.record_type != "message" {
            continue;
        }

        let Some(role) = entry.role.as_deref() else {
            continue;
        };
        if role != "user" && role != "assistant" {
            continue;
        }

        if role == "assistant" {
            if let Some(response_id) = entry
                .response_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                last_response_id = Some(response_id.to_string());
            }
        }

        messages.push(ConversationMessage {
            id: entry.entry_id,
            role: role.to_string(),
            text: entry.text.unwrap_or_default(),
            created_at_unix_ms: entry.created_at_unix_ms,
            response_id: entry.response_id,
            context_items: entry.context_items,
            timeline_events: entry.timeline_events,
        });
    }

    Ok(Some(ConversationDetail {
        conversation_id: conversation_id.to_string(),
        title: normalized_meta_title(&data.meta),
        title_source: normalize_title_source(&data.meta.title_source),
        last_response_id,
        messages,
    }))
}

pub fn load_title_generation_candidate(
    conversation_id: &str,
) -> Result<Option<TitleGenerationCandidate>, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };

    if normalize_title_source(&data.meta.title_source) != "pending" {
        return Ok(None);
    }

    data.entries.sort_by_key(|entry| entry.created_at_unix_ms);

    let mut user_text = None;
    let mut assistant_text = None;
    for entry in data.entries {
        if entry.record_type != "message" {
            continue;
        }

        match entry.role.as_deref() {
            Some("user") if user_text.is_none() => {
                let text = entry.text.unwrap_or_default();
                if !text.trim().is_empty() {
                    user_text = Some(text);
                }
            }
            Some("assistant") if assistant_text.is_none() => {
                let text = entry.text.unwrap_or_default();
                if !text.trim().is_empty() {
                    assistant_text = Some(text);
                }
            }
            _ => {}
        }

        if user_text.is_some() && assistant_text.is_some() {
            break;
        }
    }

    let Some(user_text) = user_text else {
        return Ok(None);
    };
    let Some(assistant_text) = assistant_text else {
        return Ok(None);
    };

    Ok(Some(TitleGenerationCandidate {
        user_text,
        assistant_text,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_conversation_removes_session_directory() {
        let conversation_id = format!("test-delete-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";

        let path = conversation_dir_path(&conversation_id).expect("path");
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");
        assert!(path.exists(), "session directory should exist before delete");

        let deleted = delete_conversation(&conversation_id).expect("delete conversation");
        assert!(deleted, "delete should report true when file existed");
        assert!(!path.exists(), "session directory should be removed after delete");
    }

    #[test]
    fn load_context_merges_history_for_all_providers() {
        let conversation_id = format!("test-provider-filter-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "user-openai".to_string(),
                role: "user".to_string(),
                text: "hello openai".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: Some("openai".to_string()),
                model_profile: Some("gpt-5-mini".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-openai".to_string()),
                context_items: build_user_input_items("hello openai"),
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append openai user");

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "assistant-openai".to_string(),
                role: "assistant".to_string(),
                text: "openai answer".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-openai".to_string()),
                provider: Some("openai".to_string()),
                model_profile: Some("gpt-5-mini".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-openai".to_string()),
                context_items: build_assistant_output_items("openai answer"),
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append openai assistant");

        let openai_context = load_context_for_request(&conversation_id).expect("openai context");
        assert!(openai_context.input_items.len() >= 2);

        delete_conversation(&conversation_id).expect("cleanup conversation");
    }
}
