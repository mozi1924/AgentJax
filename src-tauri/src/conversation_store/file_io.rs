use super::types::{
    ConversationData, ConversationLine, ConversationMeta, ConversationSummary,
    DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
};
use crate::conversation_store_utils::{
    compact_preview, normalize_title_source, sanitize_conversation_id,
};
use std::collections::HashSet;
use std::fs;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;

// ── Read ──────────────────────────────────────────────────────────────────

pub fn read_conversation_file(
    metadata_path: &Path,
    messages_path: &Path,
) -> Result<Option<ConversationData>, String> {
    if !metadata_path.exists() || !messages_path.exists() {
        return Ok(None);
    }

    let Some(mut meta) = read_conversation_meta(metadata_path)? else {
        return Ok(None);
    };

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

pub fn read_conversation_meta(metadata_path: &Path) -> Result<Option<ConversationMeta>, String> {
    if !metadata_path.exists() {
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
    Ok(Some(meta))
}

// ── Write ─────────────────────────────────────────────────────────────────

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
    write_conversation_metadata(metadata_path, &data.meta)?;

    // messages.jsonl — one compact JSON line per item
    let mut buf = String::with_capacity(data.lines.len() * 256);
    for line in &data.lines {
        let json = serde_json::to_string(line)
            .map_err(|e| format!("Failed to serialize conversation line: {e}"))?;
        buf.push_str(&json);
        buf.push('\n');
    }
    write_file_atomically(messages_path, buf.as_bytes(), "messages")
}

pub fn append_conversation_line(
    messages_path: &Path,
    line: &ConversationLine,
) -> Result<(), String> {
    ensure_parent_dir(messages_path, "messages")?;

    let json = serde_json::to_string(line)
        .map_err(|e| format!("Failed to serialize conversation line: {e}"))?;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(messages_path)
        .map_err(|e| {
            format!(
                "Failed to open messages file {} for append: {e}",
                messages_path.display()
            )
        })?;
    file.write_all(json.as_bytes()).map_err(|e| {
        format!(
            "Failed to append conversation line to {}: {e}",
            messages_path.display()
        )
    })?;
    file.write_all(b"\n").map_err(|e| {
        format!(
            "Failed to append newline to messages file {}: {e}",
            messages_path.display()
        )
    })?;
    file.sync_data().map_err(|e| {
        format!(
            "Failed to sync appended messages file {}: {e}",
            messages_path.display()
        )
    })
}

pub fn write_conversation_metadata(
    metadata_path: &Path,
    meta: &ConversationMeta,
) -> Result<(), String> {
    ensure_parent_dir(metadata_path, "metadata")?;
    let meta_json = serde_json::to_string_pretty(meta)
        .map_err(|e| format!("Failed to serialize metadata: {e}"))?;
    write_file_atomically(
        metadata_path,
        format!("{meta_json}\n").as_bytes(),
        "metadata",
    )
}

pub fn read_conversation_line_ids(messages_path: &Path) -> Result<HashSet<String>, String> {
    if !messages_path.exists() {
        return Ok(HashSet::new());
    }

    let file = fs::File::open(messages_path).map_err(|e| {
        format!(
            "Failed to open messages file {} while loading line-id index: {e}",
            messages_path.display()
        )
    })?;
    let reader = BufReader::new(file);
    let mut line_ids = HashSet::new();

    for (idx, raw) in reader.lines().enumerate() {
        let raw = raw.map_err(|e| {
            format!(
                "Failed to read line {} from {} while loading line-id index: {e}",
                idx + 1,
                messages_path.display()
            )
        })?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<ConversationLine>(trimmed) {
            Ok(line) => {
                line_ids.insert(line.id().to_string());
            }
            Err(err) => {
                log::warn!(
                    "Skipping malformed line {} in {} while loading line-id index: {}",
                    idx + 1,
                    messages_path.display(),
                    err
                );
            }
        }
    }

    Ok(line_ids)
}

pub fn rewrite_conversation_messages<F>(
    messages_path: &Path,
    current_meta: &ConversationMeta,
    mut transform: F,
) -> Result<(ConversationMeta, bool, HashSet<String>), String>
where
    F: FnMut(ConversationLine) -> (ConversationLine, bool),
{
    if !messages_path.exists() {
        return Ok((current_meta.clone(), false, HashSet::new()));
    }

    let file = fs::File::open(messages_path).map_err(|e| {
        format!(
            "Failed to open messages file {} for rewrite: {e}",
            messages_path.display()
        )
    })?;
    let reader = BufReader::new(file);
    let (tmp_path, mut tmp_file) = create_temp_file(messages_path, "messages")?;

    let mut rebuilt_meta = current_meta.clone();
    sanitize_meta_basics(&mut rebuilt_meta);
    rebuilt_meta.message_count = 0;
    rebuilt_meta.last_message_preview.clear();
    let previous_updated_at = rebuilt_meta.updated_at_unix_ms;
    rebuilt_meta.updated_at_unix_ms = rebuilt_meta.created_at_unix_ms;
    let mut line_ids = HashSet::new();

    let mut transformed_any = false;
    for (idx, raw) in reader.lines().enumerate() {
        let raw = raw.map_err(|e| {
            format!(
                "Failed to read line {} from {} during rewrite: {e}",
                idx + 1,
                messages_path.display()
            )
        })?;
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }

        match serde_json::from_str::<ConversationLine>(trimmed) {
            Ok(line) => {
                let (next_line, changed) = transform(line);
                transformed_any |= changed;
                line_ids.insert(next_line.id().to_string());
                write_jsonl_line(&mut tmp_file, &next_line, messages_path, "messages")?;
                apply_line_to_meta(&mut rebuilt_meta, &next_line);
            }
            Err(err) => {
                log::warn!(
                    "Skipping malformed line {} in {} during rewrite: {}",
                    idx + 1,
                    messages_path.display(),
                    err
                );
            }
        }
    }

    tmp_file.sync_all().map_err(|e| {
        format!(
            "Failed to sync temporary messages file {}: {e}",
            tmp_path.display()
        )
    })?;
    drop(tmp_file);
    finalize_atomic_replace(&tmp_path, messages_path, "messages")?;

    rebuilt_meta.updated_at_unix_ms = rebuilt_meta.updated_at_unix_ms.max(previous_updated_at);
    Ok((rebuilt_meta, transformed_any, line_ids))
}

pub fn apply_line_to_meta(meta: &mut ConversationMeta, line: &ConversationLine) {
    meta.updated_at_unix_ms = meta.updated_at_unix_ms.max(line.ts());

    if !line.is_message() {
        return;
    }

    meta.message_count += 1;
    match line {
        ConversationLine::User(user) => {
            meta.last_message_preview = compact_preview(&user.text);
        }
        ConversationLine::Assistant(assistant) => {
            if !assistant.text.trim().is_empty() {
                meta.last_message_preview = compact_preview(&assistant.text);
            }
        }
        ConversationLine::Tool(_) => {}
    }
}

fn ensure_parent_dir(path: &Path, label: &str) -> Result<(), String> {
    let Some(parent) = path.parent() else {
        return Err(format!(
            "Failed to resolve parent directory for {} file {}",
            label,
            path.display()
        ));
    };

    if !parent.exists() {
        fs::create_dir_all(parent).map_err(|e| {
            format!(
                "Failed to create parent directory {} for {} file: {e}",
                parent.display(),
                label
            )
        })?;
    }

    Ok(())
}

fn write_file_atomically(path: &Path, contents: &[u8], label: &str) -> Result<(), String> {
    let (tmp_path, mut tmp_file) = create_temp_file(path, label)?;
    tmp_file.write_all(contents).map_err(|e| {
        format!(
            "Failed to write temporary {} file {}: {e}",
            label,
            tmp_path.display()
        )
    })?;
    tmp_file.sync_all().map_err(|e| {
        format!(
            "Failed to sync temporary {} file {}: {e}",
            label,
            tmp_path.display()
        )
    })?;
    drop(tmp_file);

    finalize_atomic_replace(&tmp_path, path, label)
}

fn create_temp_file(path: &Path, label: &str) -> Result<(std::path::PathBuf, fs::File), String> {
    let parent = path.parent().ok_or_else(|| {
        format!(
            "Failed to resolve parent directory for {} file {}",
            label,
            path.display()
        )
    })?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("Invalid {} file name {}", label, path.display()))?;
    let unique_suffix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let tmp_path = parent.join(format!(
        ".{file_name}.tmp-{}-{unique_suffix}",
        std::process::id()
    ));

    let tmp_file = fs::File::create(&tmp_path).map_err(|e| {
        format!(
            "Failed to create temporary {} file {}: {e}",
            label,
            tmp_path.display()
        )
    })?;

    Ok((tmp_path, tmp_file))
}

fn write_jsonl_line(
    file: &mut fs::File,
    line: &ConversationLine,
    path: &Path,
    label: &str,
) -> Result<(), String> {
    let json = serde_json::to_string(line)
        .map_err(|e| format!("Failed to serialize conversation line: {e}"))?;
    file.write_all(json.as_bytes()).map_err(|e| {
        format!(
            "Failed to write {} line to temporary file for {}: {e}",
            label,
            path.display()
        )
    })?;
    file.write_all(b"\n").map_err(|e| {
        format!(
            "Failed to write newline to temporary {} file for {}: {e}",
            label,
            path.display()
        )
    })
}

fn finalize_atomic_replace(tmp_path: &Path, path: &Path, label: &str) -> Result<(), String> {
    if let Err(rename_err) = fs::rename(tmp_path, path) {
        #[cfg(target_os = "windows")]
        {
            if path.exists() {
                fs::remove_file(path).map_err(|e| {
                    format!(
                        "Failed to replace existing {} file {} after rename error {}: {e}",
                        label,
                        path.display(),
                        rename_err
                    )
                })?;
                fs::rename(tmp_path, path).map_err(|e| {
                    format!(
                        "Failed to finalize {} file {} after rename retry: {e}",
                        label,
                        path.display()
                    )
                })?;
            } else {
                return Err(format!(
                    "Failed to atomically rename temporary {} file {} to {}: {}",
                    label,
                    tmp_path.display(),
                    path.display(),
                    rename_err
                ));
            }
        }

        #[cfg(not(target_os = "windows"))]
        {
            let _ = fs::remove_file(tmp_path);
            return Err(format!(
                "Failed to atomically rename temporary {} file {} to {}: {}",
                label,
                tmp_path.display(),
                path.display(),
                rename_err
            ));
        }
    }

    Ok(())
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
