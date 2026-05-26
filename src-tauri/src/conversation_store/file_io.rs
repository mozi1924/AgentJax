use super::types::{
    ConversationData, ConversationLine, ConversationMeta, ConversationSummary,
    DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
};
use crate::conversation_store_utils::{
    compact_preview, normalize_title_source, sanitize_conversation_id,
};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::Path;

// ── Read ──────────────────────────────────────────────────────────────────

pub fn read_conversation_file(
    metadata_path: &Path,
    messages_path: &Path,
) -> Result<Option<ConversationData>, String> {
    if !metadata_path.exists() || !messages_path.exists() {
        return Ok(None);
    }

    let raw_meta = fs::read_to_string(metadata_path).map_err(|e| {
        format!(
            "Failed to read metadata file {}: {e}",
            metadata_path.display()
        )
    })?;
    let mut meta: ConversationMeta = serde_json::from_str(&raw_meta).map_err(|e| {
        format!(
            "Failed to parse metadata file {}: {e}",
            metadata_path.display()
        )
    })?;
    meta.conversation_id = sanitize_conversation_id(&meta.conversation_id);
    if meta.conversation_id.is_empty() {
        return Ok(None);
    }
    sanitize_meta_basics(&mut meta);

    let file = fs::File::open(messages_path).map_err(|e| {
        format!(
            "Failed to open messages file {}: {e}",
            messages_path.display()
        )
    })?;
    let reader = BufReader::new(file);
    let mut lines = Vec::new();

    for (idx, raw) in reader.lines().enumerate() {
        let raw = raw.map_err(|e| {
            format!(
                "Failed to read line {} from {}: {e}",
                idx + 1,
                messages_path.display()
            )
        })?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<ConversationLine>(trimmed) {
            Ok(line) => lines.push(line),
            Err(err) => {
                log::warn!(
                    "Skipping malformed line {} in {}: {}",
                    idx + 1,
                    messages_path.display(),
                    err
                );
            }
        }
    }

    refresh_meta_from_lines(&mut meta, &lines);
    Ok(Some(ConversationData { meta, lines }))
}

// ── Write (full file rewrite — conversation files are small) ──────────────

pub fn write_conversation_file(
    metadata_path: &Path,
    messages_path: &Path,
    data: &ConversationData,
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

    // metadata.json — pretty-printed
    let meta_json = serde_json::to_string_pretty(&data.meta)
        .map_err(|e| format!("Failed to serialize metadata: {e}"))?;
    fs::write(metadata_path, format!("{meta_json}\n")).map_err(|e| {
        format!(
            "Failed to write metadata file {}: {e}",
            metadata_path.display()
        )
    })?;

    // messages.jsonl — one compact JSON line per item
    let mut buf = String::with_capacity(data.lines.len() * 256);
    for line in &data.lines {
        let json = serde_json::to_string(line)
            .map_err(|e| format!("Failed to serialize conversation line: {e}"))?;
        buf.push_str(&json);
        buf.push('\n');
    }
    fs::write(messages_path, buf.as_bytes()).map_err(|e| {
        format!(
            "Failed to write messages file {}: {e}",
            messages_path.display()
        )
    })
}

// ── Metadata helpers ──────────────────────────────────────────────────────

fn sanitize_meta_basics(meta: &mut ConversationMeta) {
    meta.conversation_id = sanitize_conversation_id(&meta.conversation_id);
    meta.title = normalized_title(meta);
    meta.title_source = normalize_title_source(&meta.title_source);
    meta.version = LOG_VERSION;
}

fn refresh_meta_from_lines(meta: &mut ConversationMeta, lines: &[ConversationLine]) {
    let mut message_count = 0usize;
    let mut last_preview = String::new();
    let mut last_ts = meta.created_at_unix_ms;

    for line in lines {
        if !line.is_message() {
            continue;
        }
        message_count += 1;
        last_ts = last_ts.max(line.ts());

        match line {
            ConversationLine::User(u) => {
                last_preview = compact_preview(&u.text);
            }
            ConversationLine::Assistant(a) => {
                if !a.text.trim().is_empty() {
                    last_preview = compact_preview(&a.text);
                }
            }
            _ => {}
        }
    }

    meta.message_count = message_count;
    meta.updated_at_unix_ms = meta.updated_at_unix_ms.max(last_ts);
    meta.last_message_preview = last_preview;
}

fn normalized_title(meta: &ConversationMeta) -> String {
    let trimmed = meta.title.trim();
    if trimmed.is_empty() {
        DEFAULT_CONVERSATION_TITLE.to_string()
    } else {
        trimmed.to_string()
    }
}

// ── Summary extraction ────────────────────────────────────────────────────

pub fn summary_from_meta(meta: &ConversationMeta) -> ConversationSummary {
    ConversationSummary {
        conversation_id: meta.conversation_id.clone(),
        title: normalized_title(meta),
        title_source: normalize_title_source(&meta.title_source),
        message_count: meta.message_count,
        last_message_preview: meta.last_message_preview.clone(),
        updated_at_unix_ms: meta.updated_at_unix_ms,
        conversation_type: meta.conversation_type.clone(),
    }
}
