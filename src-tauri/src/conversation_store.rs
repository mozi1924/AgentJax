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
use crate::conversation_store::types::{
    ConversationEntryLine, ConversationFileData, DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
};
use crate::conversation_store_utils::{
    normalize_title, normalize_title_source, now_unix_ms, sanitize_optional, today_utc_yyyy_mm_dd,
};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::collections::HashMap;
use std::collections::HashSet;
use std::fs;
use std::path::PathBuf;
use uuid::Uuid;

const MAX_CONTEXT_ITEMS_PER_REQUEST: usize = 200;

fn parse_context_item_tool_arguments(raw: Option<&Value>) -> Value {
    let Some(value) = raw else {
        return json!({});
    };
    match value {
        Value::Object(_) => value.clone(),
        Value::String(s) => serde_json::from_str::<Value>(s).unwrap_or_else(|_| json!({})),
        _ => json!({}),
    }
}

fn is_tool_context_item(item: &Value) -> bool {
    matches!(
        item.get("type").and_then(Value::as_str),
        Some("function_call")
            | Some("function_call_output")
            | Some("custom_tool_call")
            | Some("custom_tool_call_output")
    )
}

pub fn build_recovery_developer_note(conversation_id: &str) -> Result<Option<Value>, String> {
    #[derive(Default)]
    struct RequestRecoveryState {
        last_at_unix_ms: i64,
        has_user_message: bool,
        has_assistant_message: bool,
        tool_calls: HashMap<String, (String, Value)>,
        tool_outputs: HashSet<String>,
    }

    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };
    data.entries.sort_by_key(|entry| entry.created_at_unix_ms);

    let mut states: HashMap<String, RequestRecoveryState> = HashMap::new();

    for entry in data.entries {
        let Some(request_id) = entry
            .request_id
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(ToOwned::to_owned)
        else {
            continue;
        };

        let state = states.entry(request_id).or_default();
        state.last_at_unix_ms = state.last_at_unix_ms.max(entry.created_at_unix_ms);

        if entry.record_type == "message" {
            match entry.role.as_deref() {
                Some("user") => state.has_user_message = true,
                Some("assistant") => {
                    if entry
                        .text
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .is_some()
                    {
                        state.has_assistant_message = true;
                    }
                }
                _ => {}
            }
        }

        for item in &entry.context_items {
            let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
            match item_type {
                "function_call" | "custom_tool_call" => {
                    let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
                        continue;
                    };
                    let Some(name) = item.get("name").and_then(Value::as_str) else {
                        continue;
                    };
                    state
                        .tool_calls
                        .entry(call_id.to_string())
                        .or_insert_with(|| {
                            (
                                name.to_string(),
                                parse_context_item_tool_arguments(item.get("arguments")),
                            )
                        });
                }
                "function_call_output" | "custom_tool_call_output" => {
                    if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                        state.tool_outputs.insert(call_id.to_string());
                    }
                }
                _ => {}
            }
        }
    }

    let mut selected: Option<(String, RequestRecoveryState)> = None;
    for (request_id, state) in states {
        if !state.has_user_message {
            continue;
        }

        let unresolved_calls: Vec<_> = state
            .tool_calls
            .keys()
            .filter(|call_id| !state.tool_outputs.contains(*call_id))
            .collect();
        let incomplete = !state.has_assistant_message || !unresolved_calls.is_empty();
        if !incomplete {
            continue;
        }

        let replace = selected
            .as_ref()
            .map(|(_, prev)| state.last_at_unix_ms > prev.last_at_unix_ms)
            .unwrap_or(true);
        if replace {
            selected = Some((request_id, state));
        }
    }

    let Some((request_id, state)) = selected else {
        return Ok(None);
    };

    let mut unresolved = Vec::new();
    for (call_id, (name, arguments)) in state.tool_calls {
        if state.tool_outputs.contains(&call_id) {
            continue;
        }
        unresolved.push(json!({
            "call_id": call_id,
            "tool": name,
            "arguments": arguments,
        }));
    }

    let payload = json!({
        "recovery_type": "unfinished_turn",
        "request_id": request_id,
        "assistant_message_missing": !state.has_assistant_message,
        "unresolved_tool_calls": unresolved,
    });
    let note = format!(
        "RECOVERY_CONTEXT {}\nPrevious request was interrupted. Continue from this state, avoid repeating completed steps, and resolve unresolved tool calls if still needed.",
        serde_json::to_string(&payload).unwrap_or_else(|_| "{}".to_string())
    );

    Ok(Some(json!({
        "role": "developer",
        "content": [{
            "type": "input_text",
            "text": note
        }]
    })))
}

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

pub fn append_context_item(
    input: AppendContextItemInput,
    utility_model: &str,
) -> Result<(), String> {
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

    context.input_items = sanitize_tool_call_pairs(context.input_items);
    context.input_items = truncate_context_items_preserving_tool_pairs(
        context.input_items,
        MAX_CONTEXT_ITEMS_PER_REQUEST,
    );
    Ok(context)
}

fn sanitize_tool_call_pairs(items: Vec<Value>) -> Vec<Value> {
    let mut function_call_ids = HashSet::new();
    let mut function_call_output_ids = HashSet::new();
    let mut custom_call_ids = HashSet::new();
    let mut custom_call_output_ids = HashSet::new();

    for item in &items {
        match item.get("type").and_then(Value::as_str) {
            Some("function_call") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    function_call_ids.insert(call_id.to_string());
                }
            }
            Some("function_call_output") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    function_call_output_ids.insert(call_id.to_string());
                }
            }
            Some("custom_tool_call") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    custom_call_ids.insert(call_id.to_string());
                }
            }
            Some("custom_tool_call_output") => {
                if let Some(call_id) = item.get("call_id").and_then(Value::as_str) {
                    custom_call_output_ids.insert(call_id.to_string());
                }
            }
            _ => {}
        }
    }

    items
        .into_iter()
        .filter(|item| match item.get("type").and_then(Value::as_str) {
            Some("function_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| function_call_output_ids.contains(call_id))
                .unwrap_or(false),
            Some("function_call_output") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| function_call_ids.contains(call_id))
                .unwrap_or(false),
            Some("custom_tool_call") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| custom_call_output_ids.contains(call_id))
                .unwrap_or(false),
            Some("custom_tool_call_output") => item
                .get("call_id")
                .and_then(Value::as_str)
                .map(|call_id| custom_call_ids.contains(call_id))
                .unwrap_or(false),
            _ => true,
        })
        .collect()
}

fn truncate_context_items_preserving_tool_pairs(items: Vec<Value>, max_items: usize) -> Vec<Value> {
    if items.len() <= max_items {
        return items;
    }

    #[derive(Debug, Clone, PartialEq, Eq, Hash)]
    enum ToolKind {
        Function,
        Custom,
    }

    let mut groups: HashMap<(ToolKind, String), Vec<usize>> = HashMap::new();
    for (idx, item) in items.iter().enumerate() {
        let kind = match item.get("type").and_then(Value::as_str) {
            Some("function_call") | Some("function_call_output") => Some(ToolKind::Function),
            Some("custom_tool_call") | Some("custom_tool_call_output") => Some(ToolKind::Custom),
            _ => None,
        };
        let Some(kind) = kind else {
            continue;
        };
        let Some(call_id) = item.get("call_id").and_then(Value::as_str) else {
            continue;
        };
        groups
            .entry((kind, call_id.to_string()))
            .or_default()
            .push(idx);
    }

    let mut selected = vec![false; items.len()];
    let mut kept = 0usize;

    for idx in (0..items.len()).rev() {
        if selected[idx] {
            continue;
        }

        let group_indices: Vec<usize> = match items[idx].get("type").and_then(Value::as_str) {
            Some("function_call") | Some("function_call_output") => {
                match items[idx].get("call_id").and_then(Value::as_str) {
                    Some(call_id) => groups
                        .get(&(ToolKind::Function, call_id.to_string()))
                        .cloned()
                        .unwrap_or_else(|| vec![idx]),
                    None => vec![idx],
                }
            }
            Some("custom_tool_call") | Some("custom_tool_call_output") => {
                match items[idx].get("call_id").and_then(Value::as_str) {
                    Some(call_id) => groups
                        .get(&(ToolKind::Custom, call_id.to_string()))
                        .cloned()
                        .unwrap_or_else(|| vec![idx]),
                    None => vec![idx],
                }
            }
            _ => vec![idx],
        };

        let new_items = group_indices
            .iter()
            .filter(|&&group_idx| !selected[group_idx])
            .count();
        if kept + new_items > max_items {
            continue;
        }

        for group_idx in group_indices {
            if !selected[group_idx] {
                selected[group_idx] = true;
                kept += 1;
            }
        }

        if kept >= max_items {
            break;
        }
    }

    items
        .into_iter()
        .enumerate()
        .filter_map(|(idx, item)| if selected[idx] { Some(item) } else { None })
        .collect()
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
        if entry.record_type == "context_item" {
            if entry.context_items.iter().any(is_tool_context_item) {
                messages.push(ConversationMessage {
                    id: entry.entry_id,
                    role: "assistant".to_string(),
                    text: String::new(),
                    created_at_unix_ms: entry.created_at_unix_ms,
                    response_id: entry.response_id,
                    context_items: entry.context_items,
                    timeline_events: None,
                });
            }
            continue;
        }

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
    use crate::agentjax_home::AGENTJAX_HOME_ENV;

    struct TestHomeGuard {
        home: std::path::PathBuf,
    }

    impl Drop for TestHomeGuard {
        fn drop(&mut self) {
            unsafe {
                std::env::remove_var(AGENTJAX_HOME_ENV);
            }
            let _ = std::fs::remove_dir_all(&self.home);
        }
    }

    fn setup_test_home() -> TestHomeGuard {
        let home = std::env::temp_dir().join(format!(
            "agentjax-conversation-store-test-{}",
            Uuid::new_v4()
        ));
        std::fs::create_dir_all(&home).expect("create test home");
        unsafe {
            std::env::set_var(AGENTJAX_HOME_ENV, &home);
        }
        TestHomeGuard { home }
    }

    #[test]
    fn delete_conversation_removes_session_directory() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-delete-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";

        let path = conversation_dir_path(&conversation_id).expect("path");
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");
        assert!(
            path.exists(),
            "session directory should exist before delete"
        );

        let deleted = delete_conversation(&conversation_id).expect("delete conversation");
        assert!(deleted, "delete should report true when file existed");
        assert!(
            !path.exists(),
            "session directory should be removed after delete"
        );
    }

    #[test]
    fn load_context_merges_history_for_all_providers() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
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

    #[test]
    fn load_context_filters_orphan_tool_call_items() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-orphan-tool-items-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        let context_items = vec![
            json!({"type":"function_call","call_id":"call_orphan","name":"tool_a","arguments":{}}),
            json!({"type":"function_call","call_id":"call_ok","name":"tool_b","arguments":{}}),
            json!({"type":"function_call_output","call_id":"call_ok","output":"{\"ok\":true}"}),
        ];

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "assistant-tool-history".to_string(),
                role: "assistant".to_string(),
                text: "done".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-tool".to_string()),
                provider: Some("openai".to_string()),
                model_profile: Some("gpt-5-mini".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-tool".to_string()),
                context_items,
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append assistant");

        let context = load_context_for_request(&conversation_id).expect("context");
        assert!(
            !context.input_items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call")
                    && item.get("call_id").and_then(Value::as_str) == Some("call_orphan")
            }),
            "orphan function_call should be filtered"
        );
        assert!(
            context.input_items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call")
                    && item.get("call_id").and_then(Value::as_str) == Some("call_ok")
            }),
            "paired function_call should be kept"
        );
        assert!(
            context.input_items.iter().any(|item| {
                item.get("type").and_then(Value::as_str) == Some("function_call_output")
                    && item.get("call_id").and_then(Value::as_str) == Some("call_ok")
            }),
            "paired function_call_output should be kept"
        );

        delete_conversation(&conversation_id).expect("cleanup");
    }

    #[test]
    fn load_context_truncates_without_splitting_tool_pairs() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-context-truncate-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        let mut context_items = Vec::new();
        for i in 0..260 {
            context_items.push(json!({
                "role":"user",
                "content":[{"type":"input_text","text": format!("u-{i}")}]
            }));
        }
        context_items.push(
            json!({"type":"function_call","call_id":"call_tail","name":"tool_x","arguments":{}}),
        );
        context_items.push(
            json!({"type":"function_call_output","call_id":"call_tail","output":"{\"ok\":true}"}),
        );

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "assistant-long-history".to_string(),
                role: "assistant".to_string(),
                text: "done".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-long".to_string()),
                provider: Some("openai".to_string()),
                model_profile: Some("gpt-5-mini".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-long".to_string()),
                context_items,
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append");

        let context = load_context_for_request(&conversation_id).expect("context");
        assert!(context.input_items.len() <= MAX_CONTEXT_ITEMS_PER_REQUEST);
        assert!(context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some("call_tail")
        }));
        assert!(context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call_output")
                && item.get("call_id").and_then(Value::as_str) == Some("call_tail")
        }));

        delete_conversation(&conversation_id).expect("cleanup");
    }

    #[test]
    fn append_context_item_keeps_tool_pairs_across_separate_lines() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-context-item-lines-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        append_context_item(
            AppendContextItemInput {
                conversation_id: conversation_id.clone(),
                entry_id: "ctx-call".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: None,
                model_profile: None,
                model_id: None,
                request_id: Some("req-1".to_string()),
                context_item: json!({
                    "type":"function_call",
                    "call_id":"call_1",
                    "name":"mcp__demo__tool",
                    "arguments":"{\"x\":1}"
                }),
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append function_call line");

        append_context_item(
            AppendContextItemInput {
                conversation_id: conversation_id.clone(),
                entry_id: "ctx-output".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: None,
                model_profile: None,
                model_id: None,
                request_id: Some("req-1".to_string()),
                context_item: json!({
                    "type":"function_call_output",
                    "call_id":"call_1",
                    "output":"{\"ok\":true}"
                }),
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append function_call_output line");

        let context = load_context_for_request(&conversation_id).expect("load context");
        let has_call = context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call")
                && item.get("call_id").and_then(Value::as_str) == Some("call_1")
        });
        let has_output = context.input_items.iter().any(|item| {
            item.get("type").and_then(Value::as_str) == Some("function_call_output")
                && item.get("call_id").and_then(Value::as_str) == Some("call_1")
        });
        assert!(
            has_call && has_output,
            "expected tool pair restored from separate lines"
        );
    }

    #[test]
    fn build_recovery_note_for_unfinished_turn() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-recovery-note-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "msg-user-1".to_string(),
                role: "user".to_string(),
                text: "请继续".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: None,
                model_profile: None,
                model_id: None,
                request_id: Some("req-recover".to_string()),
                context_items: build_user_input_items("请继续"),
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append user");

        append_context_item(
            AppendContextItemInput {
                conversation_id: conversation_id.clone(),
                entry_id: "ctx-call-recover".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: None,
                model_profile: None,
                model_id: None,
                request_id: Some("req-recover".to_string()),
                context_item: json!({
                    "type":"function_call",
                    "call_id":"call_recover_1",
                    "name":"mcp__demo__tool",
                    "arguments":"{\"x\":1}"
                }),
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append call");

        let note = build_recovery_developer_note(&conversation_id)
            .expect("build recovery note")
            .expect("expected recovery note");
        let note_text = note
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(note_text.contains("RECOVERY_CONTEXT"));
        assert!(note_text.contains("req-recover"));
        assert!(note_text.contains("call_recover_1"));
    }

    #[test]
    fn load_conversation_includes_tool_context_item_lines() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-load-context-lines-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        append_context_item(
            AppendContextItemInput {
                conversation_id: conversation_id.clone(),
                entry_id: "ctx-tool-line".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-1".to_string()),
                provider: None,
                model_profile: None,
                model_id: None,
                request_id: Some("req-ctx-line".to_string()),
                context_item: json!({
                    "type":"function_call_output",
                    "call_id":"call_ctx_1",
                    "output":"{\"ok\":true}"
                }),
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append context item");

        let detail = load_conversation(&conversation_id)
            .expect("load conversation")
            .expect("detail");
        assert!(detail.messages.iter().any(|msg| {
            msg.id == "ctx-tool-line"
                && msg.role == "assistant"
                && msg.context_items.iter().any(|item| {
                    item.get("type").and_then(Value::as_str) == Some("function_call_output")
                })
        }));
    }

    #[test]
    fn fault_injection_interrupted_tool_turn_can_recover_and_clear() {
        let _guard = crate::config::test_env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _home = setup_test_home();
        let conversation_id = format!("test-fault-injection-{}", Uuid::new_v4());
        let utility_model = "gpt-5-mini";
        ensure_conversation(&conversation_id, utility_model).expect("ensure conversation");

        // Step 1: user message persisted, tool_call persisted, then crash before tool_output/assistant.
        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "msg-user-fault".to_string(),
                role: "user".to_string(),
                text: "请调用工具然后继续".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: Some("codex".to_string()),
                model_profile: Some("default".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-fault".to_string()),
                context_items: build_user_input_items("请调用工具然后继续"),
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append user");

        append_context_item(
            AppendContextItemInput {
                conversation_id: conversation_id.clone(),
                entry_id: "ctx-call-fault".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: None,
                provider: Some("codex".to_string()),
                model_profile: Some("default".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-fault".to_string()),
                context_item: json!({
                    "type":"function_call",
                    "call_id":"call_fault_1",
                    "name":"mcp__demo__search",
                    "arguments":"{\"q\":\"agentjax\"}"
                }),
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append function_call");

        // Step 2: restart path should emit recovery note with unresolved tool call.
        let note_before_resume = build_recovery_developer_note(&conversation_id)
            .expect("build recovery note before resume")
            .expect("expected recovery note before resume");
        let note_text = note_before_resume
            .get("content")
            .and_then(Value::as_array)
            .and_then(|arr| arr.first())
            .and_then(|part| part.get("text"))
            .and_then(Value::as_str)
            .unwrap_or("");
        assert!(note_text.contains("RECOVERY_CONTEXT"));
        assert!(note_text.contains("call_fault_1"));
        assert!(note_text.contains("assistant_message_missing"));

        // Step 3: resume execution completes tool_output and assistant.
        append_context_item(
            AppendContextItemInput {
                conversation_id: conversation_id.clone(),
                entry_id: "ctx-output-fault".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-fault-1".to_string()),
                provider: Some("codex".to_string()),
                model_profile: Some("default".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-fault".to_string()),
                context_item: json!({
                    "type":"function_call_output",
                    "call_id":"call_fault_1",
                    "output":"{\"ok\":true,\"result\":{\"hits\":3}}"
                }),
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append function_call_output");

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "msg-assistant-fault".to_string(),
                role: "assistant".to_string(),
                text: "工具执行完成，继续回答。".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-fault-1".to_string()),
                provider: Some("codex".to_string()),
                model_profile: Some("default".to_string()),
                model_id: Some("gpt-5-mini".to_string()),
                request_id: Some("req-fault".to_string()),
                context_items: build_assistant_output_items("工具执行完成，继续回答。"),
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append assistant");

        let note_after_resume =
            build_recovery_developer_note(&conversation_id).expect("build recovery after resume");
        assert!(
            note_after_resume.is_none(),
            "recovery note should be cleared after tool output and assistant message are present"
        );
    }
}
