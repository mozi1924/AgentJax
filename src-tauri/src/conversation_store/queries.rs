use super::file_io::{read_conversation_file, read_conversation_meta, summary_from_meta};
use super::locks::{cached_summary, replace_cached_summary, with_conversation_lock};
use super::paths::{conversation_messages_path, conversation_metadata_path, list_conversation_ids};
use super::types::{
    CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY, CONVERSATION_MOUNTED_MCP_SERVERS_METADATA_KEY,
    CONVERSATION_MOUNTED_TOOL_SOURCES_METADATA_KEY, CONVERSATION_TOKEN_USAGE_METADATA_KEY,
    ConversationDetail, ConversationDynamicTool, ConversationLine, ConversationMeta,
    ConversationMountedMcpServer, ConversationMountedToolDefinition, ConversationMountedToolSource,
    ConversationSummary, TitleGenerationCandidate,
};
use serde_json::Value;

fn token_count_from_usage_value(value: &Value) -> Option<usize> {
    value
        .get("totalTokens")
        .and_then(Value::as_u64)
        .or_else(|| value.get("total_tokens").and_then(Value::as_u64))
        .and_then(|count| usize::try_from(count).ok())
        .filter(|count| *count > 0)
}

fn token_usage_count_from_meta(meta: &ConversationMeta) -> Option<usize> {
    let usage = meta.metadata.get(CONVERSATION_TOKEN_USAGE_METADATA_KEY)?;

    // Fast path for the current schema: top-level `totalTokens` tracks the
    // latest provider response, which is the value that matches gateway logs.
    if let Some(count) = token_count_from_usage_value(usage) {
        return Some(count);
    }

    // Defensive fallback for possible older/debug schemas.
    usage
        .get("aggregateUsage")
        .and_then(token_count_from_usage_value)
        .or_else(|| {
            usage
                .get("hops")
                .and_then(Value::as_array)
                .and_then(|hops| hops.last())
                .and_then(token_count_from_usage_value)
        })
}

pub fn load_conversation_token_usage_count(conversation_id: &str) -> crate::error::AgentJaxResult<Option<usize>> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let Some(meta) = read_conversation_meta(&metadata_path)? else {
            return Ok(None);
        };
        Ok(token_usage_count_from_meta(&meta))
    })
}

// ── List all conversations ────────────────────────────────────────────────

pub fn list_conversations() -> crate::error::AgentJaxResult<Vec<ConversationSummary>> {
    let mut out = Vec::new();

    for conversation_id in list_conversation_ids()? {
        if let Some(summary) = with_conversation_lock(&conversation_id, || {
            if let Some(summary) = cached_summary(&conversation_id)? {
                return Ok(Some(summary));
            }

            let metadata_path = conversation_metadata_path(&conversation_id)?;
            let meta = if let Some(meta) = read_conversation_meta(&metadata_path)? {
                meta
            } else if let Some(meta) = try_load_meta_from_lcm(&conversation_id, &metadata_path)? {
                meta
            } else {
                return Ok(None);
            };
            let summary = summary_from_meta(&meta);
            replace_cached_summary(&conversation_id, summary.clone())?;
            Ok(Some(summary))
        })? {
            out.push(summary);
        }
    }

    out.sort_by_key(|b| std::cmp::Reverse(b.updated_at_unix_ms));
    Ok(out)
}

/// Try to load conversation metadata from the LCM store when `metadata.json`
/// does not exist (LCM-only conversations).
fn try_load_meta_from_lcm(
    conversation_id: &str,
    metadata_path: &std::path::Path,
) -> crate::error::AgentJaxResult<Option<ConversationMeta>> {
    let db_path = metadata_path
        .parent()
        .ok_or_else(|| "Invalid metadata path".to_string())?
        .join("lcm.db");

    if !db_path.exists() {
        return Ok(None);
    }

    let lcm_config = crate::lcm::LcmConfig::default();
    let store = crate::lcm::LcmStore::open(&db_path, lcm_config)
        .map_err(|e| format!("Failed to open LCM store for '{}': {}", conversation_id, e))?;

    store
        .get_conversation_meta(conversation_id)
        .map_err(|e| crate::error::AgentJaxError::internal(format!("Failed to query LCM meta for '{}': {}", conversation_id, e)).with_error_source(&e))
}

// ── Load full conversation detail ─────────────────────────────────────────

pub fn load_conversation(conversation_id: &str) -> crate::error::AgentJaxResult<Option<ConversationDetail>> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;

        // ── Try LCM store first (single source of truth) ──────────────
        if let Ok(Some(detail)) = try_load_from_lcm(conversation_id, &metadata_path) {
            return Ok(Some(detail));
        }

        // ── Fall back to legacy JSONL ─────────────────────────────────
        let messages_path = conversation_messages_path(conversation_id)?;
        let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
            return Ok(None);
        };
        replace_cached_summary(conversation_id, summary_from_meta(&data.meta))?;

        Ok(Some(ConversationDetail {
            conversation_id: data.meta.conversation_id.clone(),
            title: data.meta.title.clone(),
            title_source: data.meta.title_source.clone(),
            lines: data.lines,
            context_token_count: token_usage_count_from_meta(&data.meta).unwrap_or(0),
        }))
    })
}

/// Try to load conversation detail from the LCM immutable store.
fn try_load_from_lcm(
    conversation_id: &str,
    metadata_path: &std::path::Path,
) -> crate::error::AgentJaxResult<Option<ConversationDetail>> {
    let db_path = metadata_path
        .parent()
        .ok_or_else(|| "Invalid metadata path".to_string())?
        .join("lcm.db");

    if !db_path.exists() {
        return Ok(None);
    }

    let lcm_config = crate::lcm::LcmConfig::default();
    let store = crate::lcm::LcmStore::open(&db_path, lcm_config)
        .map_err(|e| format!("Failed to open LCM store: {e}"))?;

    let messages = store
        .get_conversation_messages(conversation_id)
        .map_err(|e| format!("Failed to read LCM messages: {e}"))?;

    if messages.is_empty() {
        return Ok(None);
    }

    // Convert LCM StoredMessages to ConversationLines.
    let lines = crate::lcm::stored_messages_to_conversation_lines(&messages);

    // Read metadata from the legacy metadata.json (title, timestamps, etc.).
    let (title, title_source, context_token_count) =
        if let Ok(Some(meta)) = read_conversation_meta(metadata_path) {
            (
                if meta.title.is_empty() || meta.title_source == "pending" {
                    "New Conversation".to_string()
                } else {
                    meta.title.clone()
                },
                meta.title_source.clone(),
                token_usage_count_from_meta(&meta).unwrap_or(0),
            )
        } else {
            ("New Conversation".to_string(), "pending".to_string(), 0usize)
        };

    Ok(Some(ConversationDetail {
        conversation_id: conversation_id.to_string(),
        title,
        title_source,
        lines,
        context_token_count,
    }))
}

// ── Load title generation candidate ───────────────────────────────────────

pub fn load_title_generation_candidate(
    conversation_id: &str,
) -> crate::error::AgentJaxResult<Option<TitleGenerationCandidate>> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let messages_path = conversation_messages_path(conversation_id)?;
        let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
            return Ok(None);
        };

        if data.meta.title_source != "pending" {
            return Ok(None);
        }

        let mut user_text = None;
        let mut assistant_text = None;
        for line in &data.lines {
            match line {
                ConversationLine::User(u) if user_text.is_none() => {
                    let text = u.text.trim();
                    if !text.is_empty() {
                        user_text = Some(text.to_string());
                    }
                }
                ConversationLine::Assistant(a)
                    if assistant_text.is_none() && a.is_final_or_unknown() =>
                {
                    let text = a.text.trim();
                    if !text.is_empty() {
                        assistant_text = Some(text.to_string());
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
    })
}

// ── Load conversation-scoped dynamic tools ───────────────────────────────

pub fn load_conversation_dynamic_tools(
    conversation_id: &str,
) -> crate::error::AgentJaxResult<Vec<ConversationDynamicTool>> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let Some(meta) = read_conversation_meta(&metadata_path)? else {
            return Ok(Vec::new());
        };

        let Some(value) = meta.metadata.get(CONVERSATION_DYNAMIC_TOOLS_METADATA_KEY) else {
            return Ok(Vec::new());
        };

        serde_json::from_value::<Vec<ConversationDynamicTool>>(value.clone())
            .map_err(|err| crate::error::AgentJaxError::internal(format!("Failed to parse conversation dynamic tools: {err}")).with_error_source(&err))
    })
}

pub fn load_conversation_mounted_tool_sources(
    conversation_id: &str,
) -> crate::error::AgentJaxResult<Vec<ConversationMountedToolSource>> {
    with_conversation_lock(conversation_id, || {
        let metadata_path = conversation_metadata_path(conversation_id)?;
        let Some(meta) = read_conversation_meta(&metadata_path)? else {
            return Ok(Vec::new());
        };

        if let Some(value) = meta
            .metadata
            .get(CONVERSATION_MOUNTED_TOOL_SOURCES_METADATA_KEY)
        {
            return serde_json::from_value::<Vec<ConversationMountedToolSource>>(value.clone())
                .map_err(|err| crate::error::AgentJaxError::internal(format!("Failed to parse mounted tool sources metadata: {err}")).with_error_source(&err));
        }

        // Fallback to legacy MCP servers key
        if let Some(value) = meta
            .metadata
            .get(CONVERSATION_MOUNTED_MCP_SERVERS_METADATA_KEY)
        {
            let legacy_servers =
                serde_json::from_value::<Vec<ConversationMountedMcpServer>>(value.clone())
                    .map_err(|err| {
                        format!("Failed to parse legacy mounted MCP server metadata: {err}")
                    })?;

            let generic_sources = legacy_servers
                .into_iter()
                .map(|server| ConversationMountedToolSource {
                    source_id: server.server_id,
                    source_type: "mcp".to_string(),
                    tools: server
                        .tools
                        .into_iter()
                        .map(|tool| ConversationMountedToolDefinition {
                            tool_name: tool.tool_name,
                            display_name: tool.display_name,
                            description: tool.description,
                            icon: tool.icon,
                            input_schema: tool.input_schema,
                        })
                        .collect(),
                })
                .collect();
            return Ok(generic_sources);
        }

        Ok(Vec::new())
    })
}
