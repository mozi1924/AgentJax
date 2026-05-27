use super::file_io::{
    append_conversation_line, apply_line_to_meta, read_conversation_file,
    read_conversation_line_ids, read_conversation_meta, rewrite_conversation_messages,
    summary_from_meta, write_conversation_file, write_conversation_metadata,
};
use super::locks::{
    cached_line_id_exists, insert_cached_line_id, invalidate_cached_conversation_index,
    replace_cached_line_ids, replace_cached_summary, with_conversation_lock,
};
use super::paths::{
    conversation_dir_path, conversation_messages_path, conversation_metadata_path,
    ensure_session_layout,
};
use super::types::{
    AppendLineInput, ConversationData, ConversationDynamicTool, ConversationLine, ConversationMeta,
    ConversationMountedMcpServer, ConversationSummary, ToolStatus, UpdateLineInput,
    CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY, CONVERSATION_MOUNTED_MCP_SERVERS_METADATA_KEY,
    DEFAULT_CONVERSATION_TITLE, LOG_VERSION,
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
        replace_cached_summary(conversation_id, summary_from_meta(&data.meta))?;
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
    replace_cached_summary(conversation_id, summary_from_meta(&data.meta))?;
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
    let mut meta = load_or_create_meta(&input.conversation_id, &metadata_path, &messages_path)?;

    // Deduplicate: skip if line with same id already exists.
    if line_id_exists(&input.conversation_id, &messages_path, input.line.id())? {
        log::warn!(
            "append_line: skipping duplicate line id={} kind={:?}",
            input.line.id(),
            std::mem::discriminant(&input.line)
        );
        return Ok(());
    }

    log::info!(
        "append_line: conv={} id={} append_only=true",
        input.conversation_id,
        input.line.id(),
    );

    append_conversation_line(&messages_path, &input.line)?;
    apply_line_to_meta(&mut meta, &input.line);
    write_conversation_metadata(&metadata_path, &meta)?;
    replace_cached_summary(&input.conversation_id, summary_from_meta(&meta))?;
    insert_cached_line_id(&input.conversation_id, input.line.id())
}

fn update_line_inner(input: UpdateLineInput) -> Result<(), String> {
    let metadata_path = conversation_metadata_path(&input.conversation_id)?;
    let messages_path = conversation_messages_path(&input.conversation_id)?;
    let current_meta = load_or_create_meta(&input.conversation_id, &metadata_path, &messages_path)?;
    let replacement = input.line.clone();
    let line_id = input.line_id.clone();

    let (next_meta, _updated, line_ids) =
        rewrite_conversation_messages(&messages_path, &current_meta, move |line| {
            if line.id() == line_id {
                (merge_updated_line(&line, replacement.clone()), true)
            } else {
                (line, false)
            }
        })?;

    write_conversation_metadata(&metadata_path, &next_meta)?;
    replace_cached_summary(&input.conversation_id, summary_from_meta(&next_meta))?;
    replace_cached_line_ids(&input.conversation_id, line_ids)
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
    let mut meta = load_or_create_meta(conversation_id, &metadata_path, &messages_path)?;

    meta.title = normalize_title(title);
    meta.title_source = "manual".to_string();
    meta.updated_at_unix_ms = now_unix_ms();
    write_conversation_metadata(&metadata_path, &meta)?;
    let summary = summary_from_meta(&meta);
    replace_cached_summary(conversation_id, summary.clone())?;
    Ok(summary)
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

// ── Conversation-scoped dynamic tools ────────────────────────────────────

pub fn update_conversation_dynamic_tools(
    conversation_id: &str,
    tools: Vec<ConversationDynamicTool>,
) -> Result<(), String> {
    with_conversation_lock(conversation_id, || {
        update_conversation_dynamic_tools_inner(conversation_id, tools)
    })
}

fn update_conversation_dynamic_tools_inner(
    conversation_id: &str,
    tools: Vec<ConversationDynamicTool>,
) -> Result<(), String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let mut meta = load_or_create_meta(conversation_id, &metadata_path, &messages_path)?;

    if tools.is_empty() {
        meta.metadata
            .remove(CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY);
    } else {
        let value = serde_json::to_value(&tools)
            .map_err(|err| format!("Failed to serialize dynamic tools metadata: {err}"))?;
        meta.metadata
            .insert(CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY.to_string(), value);
    }
    meta.updated_at_unix_ms = now_unix_ms();
    write_conversation_metadata(&metadata_path, &meta)?;
    replace_cached_summary(conversation_id, summary_from_meta(&meta))?;
    Ok(())
}

pub fn upsert_conversation_dynamic_tool(
    conversation_id: &str,
    tool: ConversationDynamicTool,
) -> Result<(), String> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let messages_path = conversation_messages_path(conversation_id)?;
        let meta = load_or_create_meta(conversation_id, &metadata_path, &messages_path)?;
        let mut tools = meta
            .metadata
            .get(CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY)
            .cloned()
            .map(serde_json::from_value::<Vec<ConversationDynamicTool>>)
            .transpose()
            .map_err(|err| format!("Failed to parse stored dynamic tools: {err}"))?
            .unwrap_or_default();

        if let Some(existing) = tools.iter_mut().find(|existing| existing.name == tool.name) {
            *existing = tool;
        } else {
            tools.push(tool);
        }

        update_conversation_dynamic_tools_inner(conversation_id, tools)
    })
}

pub fn remove_conversation_dynamic_tool(
    conversation_id: &str,
    tool_name: &str,
) -> Result<(), String> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let messages_path = conversation_messages_path(conversation_id)?;
        let meta = load_or_create_meta(conversation_id, &metadata_path, &messages_path)?;
        let mut tools = meta
            .metadata
            .get(CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY)
            .cloned()
            .map(serde_json::from_value::<Vec<ConversationDynamicTool>>)
            .transpose()
            .map_err(|err| format!("Failed to parse stored dynamic tools: {err}"))?
            .unwrap_or_default();

        tools.retain(|tool| tool.name != tool_name);
        update_conversation_dynamic_tools_inner(conversation_id, tools)
    })
}

pub fn update_conversation_mounted_mcp_servers(
    conversation_id: &str,
    servers: Vec<ConversationMountedMcpServer>,
) -> Result<(), String> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let messages_path = conversation_messages_path(conversation_id)?;
        let mut meta = load_or_create_meta(conversation_id, &metadata_path, &messages_path)?;

        if servers.is_empty() {
            meta.metadata
                .remove(CONVERSATION_MOUNTED_MCP_SERVERS_METADATA_KEY);
        } else {
            let value = serde_json::to_value(&servers)
                .map_err(|err| format!("Failed to serialize mounted MCP server metadata: {err}"))?;
            meta.metadata.insert(
                CONVERSATION_MOUNTED_MCP_SERVERS_METADATA_KEY.to_string(),
                value,
            );
        }

        meta.updated_at_unix_ms = now_unix_ms();
        write_conversation_metadata(&metadata_path, &meta)?;
        replace_cached_summary(conversation_id, summary_from_meta(&meta))?;
        Ok(())
    })
}

fn update_auto_title_inner(
    conversation_id: &str,
    title: &str,
) -> Result<Option<ConversationSummary>, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(mut meta) = load_existing_meta(conversation_id, &metadata_path, &messages_path)?
    else {
        return Ok(None);
    };

    if meta.title_source == "manual" {
        let summary = summary_from_meta(&meta);
        replace_cached_summary(conversation_id, summary.clone())?;
        return Ok(Some(summary));
    }

    meta.title = normalize_title(title);
    meta.title_source = "auto".to_string();
    meta.updated_at_unix_ms = now_unix_ms();
    write_conversation_metadata(&metadata_path, &meta)?;
    let summary = summary_from_meta(&meta);
    replace_cached_summary(conversation_id, summary.clone())?;
    Ok(Some(summary))
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
        invalidate_cached_conversation_index(conversation_id)?;
        return Ok(false);
    }

    fs::remove_dir_all(&dir)
        .map_err(|e| format!("Failed to delete session dir {}: {e}", dir.display()))?;
    invalidate_cached_conversation_index(conversation_id)?;
    Ok(true)
}

fn load_or_create_meta(
    conversation_id: &str,
    metadata_path: &std::path::Path,
    messages_path: &std::path::Path,
) -> Result<ConversationMeta, String> {
    if let Some(meta) = read_conversation_meta(metadata_path)? {
        return Ok(meta);
    }

    if messages_path.exists() {
        let Some(data) = read_conversation_file(metadata_path, messages_path)? else {
            return ensure_conversation_inner(conversation_id);
        };
        return Ok(data.meta);
    }

    ensure_conversation_inner(conversation_id)
}

fn load_existing_meta(
    conversation_id: &str,
    metadata_path: &std::path::Path,
    messages_path: &std::path::Path,
) -> Result<Option<ConversationMeta>, String> {
    if let Some(meta) = read_conversation_meta(metadata_path)? {
        return Ok(Some(meta));
    }

    if messages_path.exists() {
        let Some(data) = read_conversation_file(metadata_path, messages_path)? else {
            return Ok(None);
        };
        return Ok(Some(data.meta));
    }

    let _ = conversation_id;
    Ok(None)
}

fn line_id_exists(
    conversation_id: &str,
    messages_path: &std::path::Path,
    line_id: &str,
) -> Result<bool, String> {
    if let Some(exists) = cached_line_id_exists(conversation_id, line_id)? {
        return Ok(exists);
    }

    let line_ids = read_conversation_line_ids(messages_path)?;
    let exists = line_ids.contains(line_id);
    replace_cached_line_ids(conversation_id, line_ids)?;
    Ok(exists)
}

fn merge_updated_line(existing: &ConversationLine, next: ConversationLine) -> ConversationLine {
    match (existing, next) {
        (ConversationLine::Tool(current), ConversationLine::Tool(mut updated)) => {
            if updated.started_ts <= 0 {
                updated.started_ts = current.started_ts.max(current.ts);
            }
            if updated.completed_ts.is_none() {
                updated.completed_ts = current.completed_ts;
            }
            if updated.display_name.is_none() {
                updated.display_name = current.display_name.clone();
            }
            if updated.description.is_none() {
                updated.description = current.description.clone();
            }
            if updated.icon.is_none() {
                updated.icon = current.icon.clone();
            }
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
