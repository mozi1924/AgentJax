use super::file_io::{read_conversation_file, summary_from_meta, write_conversation_file};
use super::locks::with_conversation_lock;
use super::paths::{
    conversation_dir_path, conversation_messages_path, conversation_metadata_path,
    ensure_session_layout,
};
use super::types::{
    AppendLineInput, ConversationData, ConversationLine, ConversationMeta, ConversationSummary,
    ToolStatus, UpdateLineInput, DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
};
use crate::conversation_store_utils::{normalize_title, now_unix_ms};
use std::collections::BTreeMap;
use std::fs;

// ── Ensure existence ──────────────────────────────────────────────────────

pub fn ensure_conversation(conversation_id: &str) -> Result<ConversationMeta, String> {
    with_conversation_lock(conversation_id, || {
        ensure_conversation_inner(conversation_id)
    })
}

fn ensure_conversation_inner(conversation_id: &str) -> Result<ConversationMeta, String> {
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
        if changed {
            write_conversation_file(&metadata_path, &messages_path, &data)?;
        }
        return Ok(data.meta);
    }

    let now = now_unix_ms();
    let data = ConversationData {
        meta: ConversationMeta {
            version: LOG_VERSION,
            conversation_id: conversation_id.to_string(),
            created_at_unix_ms: now,
            updated_at_unix_ms: now,
            title: DEFAULT_CONVERSATION_TITLE.to_string(),
            title_source: "pending".to_string(),
            message_count: 0,
            last_message_preview: String::new(),
            conversation_type: String::new(),
            metadata: BTreeMap::new(),
        },
        lines: Vec::new(),
    };

    ensure_session_layout(conversation_id)?;
    write_conversation_file(&metadata_path, &messages_path, &data)?;
    Ok(data.meta)
}

// ── Append line ───────────────────────────────────────────────────────────

pub fn append_line(input: AppendLineInput) -> Result<(), String> {
    let conversation_id = input.conversation_id.clone();
    with_conversation_lock(&conversation_id, move || append_line_inner(input))
}

// ── Update existing line (in-place replace by id) ─────────────────────────

pub fn update_line(input: UpdateLineInput) -> Result<(), String> {
    let conversation_id = input.conversation_id.clone();
    with_conversation_lock(&conversation_id, move || update_line_inner(input))
}

fn append_line_inner(input: AppendLineInput) -> Result<(), String> {
    let metadata_path = conversation_metadata_path(&input.conversation_id)?;
    let messages_path = conversation_messages_path(&input.conversation_id)?;
    let mut data = load_or_create_inner(&input.conversation_id, &metadata_path, &messages_path)?;

    // Deduplicate: skip if line with same id already exists.
    if data.lines.iter().any(|l| l.id() == input.line.id()) {
        log::warn!(
            "append_line: skipping duplicate line id={} kind={:?}",
            input.line.id(),
            std::mem::discriminant(&input.line)
        );
        return Ok(());
    }

    log::info!(
        "append_line: conv={} id={} lines_before={}",
        input.conversation_id,
        input.line.id(),
        data.lines.len()
    );

    data.lines.push(input.line);
    write_conversation_file(&metadata_path, &messages_path, &data)
}

fn update_line_inner(input: UpdateLineInput) -> Result<(), String> {
    let metadata_path = conversation_metadata_path(&input.conversation_id)?;
    let messages_path = conversation_messages_path(&input.conversation_id)?;
    let mut data = load_or_create_inner(&input.conversation_id, &metadata_path, &messages_path)?;

    if let Some(existing) = data.lines.iter_mut().find(|l| l.id() == input.line_id) {
        *existing = merge_updated_line(existing, input.line);
    }

    write_conversation_file(&metadata_path, &messages_path, &data)
}

// ── Rename ────────────────────────────────────────────────────────────────

pub fn rename_conversation(
    conversation_id: &str,
    title: &str,
) -> Result<ConversationSummary, String> {
    with_conversation_lock(conversation_id, || {
        rename_conversation_inner(conversation_id, title)
    })
}

fn rename_conversation_inner(
    conversation_id: &str,
    title: &str,
) -> Result<ConversationSummary, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let mut data = load_or_create_inner(conversation_id, &metadata_path, &messages_path)?;

    data.meta.title = normalize_title(title);
    data.meta.title_source = "manual".to_string();
    data.meta.updated_at_unix_ms = now_unix_ms();
    write_conversation_file(&metadata_path, &messages_path, &data)?;
    Ok(summary_from_meta(&data.meta))
}

// ── Auto-title update ─────────────────────────────────────────────────────

pub fn update_auto_title(
    conversation_id: &str,
    title: &str,
) -> Result<Option<ConversationSummary>, String> {
    with_conversation_lock(conversation_id, || {
        update_auto_title_inner(conversation_id, title)
    })
}

fn update_auto_title_inner(
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
    write_conversation_file(&metadata_path, &messages_path, &data)?;
    Ok(Some(summary_from_meta(&data.meta)))
}

// ── Delete ────────────────────────────────────────────────────────────────

pub fn delete_conversation(conversation_id: &str) -> Result<bool, String> {
    with_conversation_lock(conversation_id, || {
        delete_conversation_inner(conversation_id)
    })
}

fn delete_conversation_inner(conversation_id: &str) -> Result<bool, String> {
    let dir = conversation_dir_path(conversation_id)?;
    if !dir.exists() {
        return Ok(false);
    }

    fs::remove_dir_all(&dir)
        .map_err(|e| format!("Failed to delete session dir {}: {e}", dir.display()))?;
    Ok(true)
}

// ── Internal helper ───────────────────────────────────────────────────────

fn load_or_create_inner(
    conversation_id: &str,
    metadata_path: &std::path::Path,
    messages_path: &std::path::Path,
) -> Result<ConversationData, String> {
    if let Some(existing) = read_conversation_file(metadata_path, messages_path)? {
        return Ok(existing);
    }
    let meta = ensure_conversation_inner(conversation_id)?;
    Ok(ConversationData {
        meta,
        lines: Vec::new(),
    })
}

fn merge_updated_line(existing: &ConversationLine, next: ConversationLine) -> ConversationLine {
    match (existing, next) {
        (ConversationLine::Tool(current), ConversationLine::Tool(mut updated)) => {
            if updated.args.is_null() {
                updated.args = current.args.clone();
            }
            if updated.output.is_none() {
                updated.output = current.output.clone();
            }
            if matches!(updated.status, ToolStatus::Pending)
                && !matches!(current.status, ToolStatus::Pending)
            {
                updated.status = current.status.clone();
            }
            ConversationLine::Tool(updated)
        }
        (_, updated) => updated,
    }
}
