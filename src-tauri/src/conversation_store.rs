use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use uuid::Uuid;
use crate::conversation_store_utils::{
    compact_preview, normalize_title, normalize_title_source, now_unix_ms, sanitize_conversation_id,
    sanitize_optional, today_utc_yyyy_mm_dd,
};

const AGENTJAX_DIR_NAME: &str = ".agentjax";
const SESSIONS_DIR_NAME: &str = "sessions";
const METADATA_FILE_NAME: &str = "metadata.json";
const MESSAGES_FILE_NAME: &str = "messages.jsonl";
const WORKSPACE_DIR_NAME: &str = "workspace";
const LOG_VERSION: u32 = 3;
const DEFAULT_CONVERSATION_TITLE: &str = "新对话";

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
struct ConversationFileData {
    meta: ConversationMetaLine,
    entries: Vec<ConversationEntryLine>,
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

pub fn conversations_dir_path() -> Result<PathBuf, String> {
    Ok(agentjax_home_dir()?.join(SESSIONS_DIR_NAME))
}

pub fn ensure_conversations_dir() -> Result<PathBuf, String> {
    let dir = conversations_dir_path()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create conversations dir {}: {e}", dir.display()))?;
    }
    Ok(dir)
}

pub fn conversation_dir_path(conversation_id: &str) -> Result<PathBuf, String> {
    let dir = ensure_conversations_dir()?;
    let safe = sanitize_conversation_id(conversation_id);
    Ok(dir.join(safe))
}

fn conversation_metadata_path(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversation_dir_path(conversation_id)?.join(METADATA_FILE_NAME))
}

fn conversation_messages_path(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversation_dir_path(conversation_id)?.join(MESSAGES_FILE_NAME))
}

pub fn conversation_workspace_path(conversation_id: &str) -> Result<PathBuf, String> {
    Ok(conversation_dir_path(conversation_id)?.join(WORKSPACE_DIR_NAME))
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

fn list_conversation_ids() -> Result<Vec<String>, String> {
    let dir = ensure_conversations_dir()?;
    let mut out = Vec::new();

    let entries = fs::read_dir(&dir)
        .map_err(|e| format!("Failed to read conversations dir {}: {e}", dir.display()))?;

    for entry in entries {
        let entry = entry.map_err(|e| format!("Failed to inspect conversation file entry: {e}"))?;
        let path = entry.path();

        if !path.is_dir() {
            continue;
        }
        let metadata = path.join(METADATA_FILE_NAME);
        let messages = path.join(MESSAGES_FILE_NAME);
        if !metadata.exists() || !messages.exists() {
            continue;
        }
        if let Some(name) = path.file_name().and_then(|s| s.to_str()) {
            if !name.trim().is_empty() {
                out.push(name.to_string());
            }
        }
    }

    Ok(out)
}

fn read_conversation_file(
    metadata_path: &Path,
    messages_path: &Path,
) -> Result<Option<ConversationFileData>, String> {
    if !metadata_path.exists() || !messages_path.exists() {
        return Ok(None);
    }

    let raw_meta = fs::read_to_string(metadata_path).map_err(|e| {
        format!(
            "Failed to open session metadata file {}: {e}",
            metadata_path.display()
        )
    })?;
    let mut meta: ConversationMetaLine = serde_json::from_str(&raw_meta).map_err(|e| {
        format!(
            "Failed to parse session metadata file {}: {e}",
            metadata_path.display()
        )
    })?;
    meta.conversation_id = sanitize_conversation_id(&meta.conversation_id);
    if meta.conversation_id.is_empty() {
        return Ok(None);
    }

    let file = fs::File::open(messages_path).map_err(|e| {
        format!(
            "Failed to open session messages file {}: {e}",
            messages_path.display()
        )
    })?;
    let reader = BufReader::new(file);

    let mut entries = Vec::new();

    for (idx, raw) in reader.lines().enumerate() {
        let raw = raw.map_err(|e| {
            format!(
                "Failed to read line {} from session messages file {}: {e}",
                idx + 1,
                messages_path.display()
            )
        })?;

        if raw.trim().is_empty() {
            continue;
        }

        let value = match serde_json::from_str::<Value>(&raw) {
            Ok(value) => value,
            Err(err) => {
                log::warn!(
                    "Skipping malformed conversation line {} in {}: {}",
                    idx + 1,
                    messages_path.display(),
                    err
                );
                continue;
            }
        };

        let record_type = value
            .get("recordType")
            .or_else(|| value.get("record_type"))
            .and_then(Value::as_str)
            .unwrap_or("");

        if record_type.is_empty() {
            continue;
        }

        match serde_json::from_value::<ConversationEntryLine>(value) {
            Ok(entry) => entries.push(entry),
            Err(err) => {
                log::warn!(
                    "Skipping malformed conversation entry line {} in {}: {}",
                    idx + 1,
                    messages_path.display(),
                    err
                );
            }
        }
    }

    refresh_meta_derived_fields(&mut meta, &entries);
    Ok(Some(ConversationFileData { meta, entries }))
}

fn write_conversation_file(
    metadata_path: &Path,
    messages_path: &Path,
    data: &ConversationFileData,
) -> Result<(), String> {
    if let Some(parent) = metadata_path.parent() {
        if !parent.exists() {
            fs::create_dir_all(parent).map_err(|e| {
                format!(
                    "Failed to create session directory {}: {e}",
                    parent.display()
                )
            })?;
        }
    }

    let mut messages_lines = Vec::with_capacity(data.entries.len());

    for entry in &data.entries {
        messages_lines.push(
            serde_json::to_string(entry)
                .map_err(|e| format!("Failed to serialize conversation entry: {e}"))?,
        );
    }

    fs::write(
        metadata_path,
        format!(
            "{}\n",
            serde_json::to_string_pretty(&data.meta)
                .map_err(|e| format!("Failed to serialize conversation metadata: {e}"))?
        ),
    )
    .map_err(|e| {
        format!(
            "Failed to write session metadata file {}: {e}",
            metadata_path.display()
        )
    })?;

    fs::write(messages_path, format!("{}\n", messages_lines.join("\n"))).map_err(|e| {
        format!(
            "Failed to write session messages file {}: {e}",
            messages_path.display()
        )
    })
}

fn refresh_meta_derived_fields(meta: &mut ConversationMetaLine, entries: &[ConversationEntryLine]) {
    meta.version = LOG_VERSION;
    meta.record_type = "meta".to_string();
    meta.conversation_id = sanitize_conversation_id(&meta.conversation_id);
    meta.title = normalized_meta_title(meta);
    meta.title_source = normalize_title_source(&meta.title_source);

    let mut message_count = 0usize;
    let mut last_message_at = 0i64;
    let mut last_message_preview = String::new();

    for entry in entries {
        if entry.record_type != "message" {
            continue;
        }

        let role = entry.role.as_deref().unwrap_or("");
        if role != "user" && role != "assistant" {
            continue;
        }

        message_count += 1;
        if entry.created_at_unix_ms >= last_message_at {
            last_message_at = entry.created_at_unix_ms;
            last_message_preview = compact_preview(entry.text.as_deref().unwrap_or(""));
        }
    }

    meta.message_count = message_count;
    meta.last_message_at_unix_ms = last_message_at;
    meta.last_message_preview = last_message_preview;
    if last_message_at > 0 {
        meta.updated_at_unix_ms = meta.updated_at_unix_ms.max(last_message_at);
    }
}

fn summary_from_meta(meta: &ConversationMetaLine) -> ConversationSummary {
    ConversationSummary {
        conversation_id: meta.conversation_id.clone(),
        title: normalized_meta_title(meta),
        title_source: normalize_title_source(&meta.title_source),
        message_count: meta.message_count,
        last_message_preview: meta.last_message_preview.clone(),
        last_message_at_unix_ms: meta.updated_at_unix_ms.max(meta.last_message_at_unix_ms),
    }
}

fn normalized_meta_title(meta: &ConversationMetaLine) -> String {
    let trimmed = meta.title.trim();
    if trimmed.is_empty() {
        DEFAULT_CONVERSATION_TITLE.to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn agentjax_home_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir()
        .ok_or_else(|| "Failed to resolve home directory for .agentjax".to_string())?;
    Ok(home.join(AGENTJAX_DIR_NAME))
}

fn ensure_session_layout(conversation_id: &str) -> Result<(), String> {
    let workspace_dir = conversation_workspace_path(conversation_id)?;
    if !workspace_dir.exists() {
        fs::create_dir_all(&workspace_dir).map_err(|e| {
            format!(
                "Failed to create session workspace directory {}: {e}",
                workspace_dir.display()
            )
        })?;
    }
    Ok(())
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

        append_message(
            AppendMessageInput {
                conversation_id: conversation_id.clone(),
                entry_id: "assistant-codex".to_string(),
                role: "assistant".to_string(),
                text: "codex answer".to_string(),
                created_at_unix_ms: now_unix_ms(),
                response_id: Some("resp-codex".to_string()),
                provider: Some("codex".to_string()),
                model_profile: Some("gpt-5.4-mini".to_string()),
                model_id: Some("gpt-5.4-mini".to_string()),
                request_id: Some("req-codex".to_string()),
                context_items: build_assistant_output_items("codex answer"),
                timeline_events: None,
                metadata: BTreeMap::new(),
            },
            utility_model,
        )
        .expect("append codex assistant");

        let openai_context = load_context_for_request(&conversation_id).expect("openai context");
        assert!(
            openai_context.input_items.len() >= 3,
            "openai context should still include shared history items"
        );

        let codex_context = load_context_for_request(&conversation_id).expect("codex context");
        assert!(
            codex_context.input_items.len() >= 3,
            "codex context should still include shared history items"
        );

        delete_conversation(&conversation_id).expect("cleanup conversation");
    }
}
