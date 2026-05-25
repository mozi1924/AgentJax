use crate::conversation_store::types::{
    ConversationEntryLine, ConversationFileData, ConversationMetaLine, ConversationSummary,
    DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
};
use crate::conversation_store_utils::{
    compact_preview, normalize_title_source, sanitize_conversation_id,
};
use serde_json::Value;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

pub fn read_conversation_file(
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

pub fn write_conversation_file(
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

pub fn refresh_meta_derived_fields(
    meta: &mut ConversationMetaLine,
    entries: &[ConversationEntryLine],
) {
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

pub fn summary_from_meta(meta: &ConversationMetaLine) -> ConversationSummary {
    ConversationSummary {
        conversation_id: meta.conversation_id.clone(),
        title: normalized_meta_title(meta),
        title_source: normalize_title_source(&meta.title_source),
        message_count: meta.message_count,
        last_message_preview: meta.last_message_preview.clone(),
        last_message_at_unix_ms: meta.updated_at_unix_ms.max(meta.last_message_at_unix_ms),
    }
}

pub fn normalized_meta_title(meta: &ConversationMetaLine) -> String {
    let trimmed = meta.title.trim();
    if trimmed.is_empty() {
        DEFAULT_CONVERSATION_TITLE.to_string()
    } else {
        trimmed.to_string()
    }
}
