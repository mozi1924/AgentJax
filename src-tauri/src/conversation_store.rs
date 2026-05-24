use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use uuid::Uuid;

use crate::config;

const CONVERSATIONS_DIR_NAME: &str = "conversations";
const LOG_VERSION: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationLogLine {
    pub version: u32,
    pub record_type: String,
    pub conversation_id: String,
    pub message_id: String,
    pub role: String,
    pub text: String,
    pub created_at_unix_ms: i64,
    pub response_id: Option<String>,
    pub provider: Option<String>,
    pub model_profile: Option<String>,
    pub model_id: Option<String>,
    pub request_id: Option<String>,
    #[serde(default)]
    pub input_items: Vec<Value>,
    #[serde(default)]
    pub output_items: Vec<Value>,
    #[serde(default)]
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone)]
pub struct AppendMessageInput {
    pub conversation_id: String,
    pub message_id: String,
    pub role: String,
    pub text: String,
    pub created_at_unix_ms: i64,
    pub response_id: Option<String>,
    pub provider: Option<String>,
    pub model_profile: Option<String>,
    pub model_id: Option<String>,
    pub request_id: Option<String>,
    pub input_items: Vec<Value>,
    pub output_items: Vec<Value>,
    pub metadata: BTreeMap<String, Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationSummary {
    pub conversation_id: String,
    pub title: String,
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
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationDetail {
    pub conversation_id: String,
    pub title: String,
    pub last_response_id: Option<String>,
    pub messages: Vec<ConversationMessage>,
}

#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    pub previous_response_id: Option<String>,
    pub input_items: Vec<Value>,
}

#[derive(Default)]
struct ConversationAggregate {
    first_user_text: Option<String>,
    last_message_preview: Option<String>,
    last_message_at_unix_ms: i64,
    message_count: usize,
}

pub fn new_conversation_id() -> String {
    Uuid::new_v4().to_string()
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
    Ok(config::config_dir_path()?.join(CONVERSATIONS_DIR_NAME))
}

pub fn ensure_conversations_dir() -> Result<PathBuf, String> {
    let _ = config::init_config_if_missing()?;
    let dir = conversations_dir_path()?;
    if !dir.exists() {
        fs::create_dir_all(&dir)
            .map_err(|e| format!("Failed to create conversations dir {}: {e}", dir.display()))?;
    }
    Ok(dir)
}

pub fn conversation_file_path(conversation_id: &str) -> Result<PathBuf, String> {
    let dir = ensure_conversations_dir()?;
    let safe = sanitize_conversation_id(conversation_id);
    Ok(dir.join(format!("{}.jsonl", safe)))
}

pub fn append_message(input: AppendMessageInput) -> Result<(), String> {
    let path = conversation_file_path(&input.conversation_id)?;

    let input_items = if input.role == "user" && input.input_items.is_empty() {
        build_user_input_items(&input.text)
    } else {
        input.input_items
    };
    let output_items = if input.role == "assistant" && input.output_items.is_empty() {
        build_assistant_output_items(&input.text)
    } else {
        input.output_items
    };

    let line = ConversationLogLine {
        version: LOG_VERSION,
        record_type: "message".to_string(),
        conversation_id: input.conversation_id,
        message_id: input.message_id,
        role: input.role,
        text: input.text,
        created_at_unix_ms: input.created_at_unix_ms,
        response_id: input.response_id,
        provider: input.provider,
        model_profile: input.model_profile,
        model_id: input.model_id,
        request_id: input.request_id,
        input_items,
        output_items,
        metadata: input.metadata,
    };

    let encoded = serde_json::to_string(&line)
        .map_err(|e| format!("Failed to serialize conversation line: {e}"))?;

    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| {
            format!(
                "Failed to open conversation file {} for append: {e}",
                path.display()
            )
        })?;

    file.write_all(encoded.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|e| format!("Failed to append to conversation file {}: {e}", path.display()))
}

pub fn load_context_for_request(conversation_id: &str) -> Result<ConversationContext, String> {
    let mut lines = read_conversation_lines(conversation_id)?;
    lines.retain(|line| line.record_type == "message");
    lines.sort_by_key(|line| line.created_at_unix_ms);

    let mut context = ConversationContext::default();

    for line in lines {
        match line.role.as_str() {
            "user" => {
                if !line.input_items.is_empty() {
                    context.input_items.extend(line.input_items);
                } else {
                    context.input_items.extend(build_user_input_items(&line.text));
                }
            }
            "assistant" => {
                if let Some(response_id) = line
                    .response_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                {
                    context.previous_response_id = Some(response_id.to_string());
                }

                if !line.output_items.is_empty() {
                    context.input_items.extend(line.output_items);
                } else {
                    context
                        .input_items
                        .extend(build_assistant_output_items(&line.text));
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
        let mut agg = ConversationAggregate::default();

        let mut lines = read_conversation_lines(&conversation_id)?;
        lines.retain(|line| line.record_type == "message");
        lines.sort_by_key(|line| line.created_at_unix_ms);

        for line in lines {
            let role = line.role.trim();
            if role != "user" && role != "assistant" {
                continue;
            }

            agg.message_count += 1;
            if role == "user" && agg.first_user_text.is_none() {
                agg.first_user_text = Some(line.text.clone());
            }
            agg.last_message_at_unix_ms = line.created_at_unix_ms;
            agg.last_message_preview = Some(line.text);
        }

        if agg.message_count == 0 {
            continue;
        }

        let title = agg
            .first_user_text
            .as_deref()
            .map(compact_title)
            .unwrap_or_else(|| conversation_id.clone());

        let preview = agg
            .last_message_preview
            .as_deref()
            .map(compact_preview)
            .unwrap_or_default();

        out.push(ConversationSummary {
            conversation_id,
            title,
            message_count: agg.message_count,
            last_message_preview: preview,
            last_message_at_unix_ms: agg.last_message_at_unix_ms,
        });
    }

    out.sort_by(|a, b| b.last_message_at_unix_ms.cmp(&a.last_message_at_unix_ms));
    Ok(out)
}

pub fn load_conversation(conversation_id: &str) -> Result<Option<ConversationDetail>, String> {
    let mut lines = read_conversation_lines(conversation_id)?;
    lines.retain(|line| line.record_type == "message");

    if lines.is_empty() {
        return Ok(None);
    }

    lines.sort_by_key(|line| line.created_at_unix_ms);

    let mut title = conversation_id.to_string();
    let mut last_response_id = None;
    let mut messages = Vec::new();

    for line in lines.into_iter() {
        let role = match line.role.as_str() {
            "user" => "user",
            "assistant" => "assistant",
            _ => continue,
        };

        if role == "user" && title == conversation_id {
            title = compact_title(&line.text);
        }

        if role == "assistant" {
            if let Some(response_id) = line
                .response_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
            {
                last_response_id = Some(response_id.to_string());
            }
        }

        messages.push(ConversationMessage {
            id: line.message_id.clone(),
            role: role.to_string(),
            text: line.text,
            created_at_unix_ms: line.created_at_unix_ms,
            response_id: line.response_id,
        });
    }

    Ok(Some(ConversationDetail {
        conversation_id: conversation_id.to_string(),
        title,
        last_response_id,
        messages,
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

        if !path.is_file() {
            continue;
        }

        if path.extension().and_then(|s| s.to_str()) != Some("jsonl") {
            continue;
        }

        if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
            if !stem.trim().is_empty() {
                out.push(stem.to_string());
            }
        }
    }

    Ok(out)
}

fn read_conversation_lines(conversation_id: &str) -> Result<Vec<ConversationLogLine>, String> {
    let path = conversation_file_path(conversation_id)?;
    read_lines_from_file(&path)
}

fn read_lines_from_file(path: &Path) -> Result<Vec<ConversationLogLine>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }

    let file = fs::File::open(path)
        .map_err(|e| format!("Failed to open conversation file {}: {e}", path.display()))?;
    let reader = BufReader::new(file);

    let mut out = Vec::new();
    for (idx, raw) in reader.lines().enumerate() {
        let raw = raw.map_err(|e| {
            format!(
                "Failed to read line {} from conversation file {}: {e}",
                idx + 1,
                path.display()
            )
        })?;

        if raw.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<ConversationLogLine>(&raw) {
            Ok(line) => out.push(line),
            Err(err) => {
                log::warn!(
                    "Skipping malformed conversation line {} in {}: {}",
                    idx + 1,
                    path.display(),
                    err
                );
            }
        }
    }

    Ok(out)
}

fn sanitize_conversation_id(conversation_id: &str) -> String {
    let trimmed = conversation_id.trim();
    let safe = trimmed
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .collect::<String>();

    if safe.is_empty() {
        "conversation".to_string()
    } else {
        safe
    }
}

fn compact_title(raw: &str) -> String {
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= 22 {
        cleaned
    } else {
        format!("{}...", cleaned.chars().take(20).collect::<String>())
    }
}

fn compact_preview(raw: &str) -> String {
    let cleaned = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if cleaned.chars().count() <= 60 {
        cleaned
    } else {
        format!("{}...", cleaned.chars().take(57).collect::<String>())
    }
}
