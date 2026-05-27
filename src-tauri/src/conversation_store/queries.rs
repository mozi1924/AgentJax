use super::file_io::{read_conversation_file, summary_from_meta};
use super::paths::{conversation_messages_path, conversation_metadata_path, list_conversation_ids};
use super::types::{
    ConversationDetail, ConversationLine, ConversationSummary, TitleGenerationCandidate,
};
// ── List all conversations ────────────────────────────────────────────────

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

    out.sort_by(|a, b| b.updated_at_unix_ms.cmp(&a.updated_at_unix_ms));
    Ok(out)
}

// ── Load full conversation detail ─────────────────────────────────────────

pub fn load_conversation(conversation_id: &str) -> Result<Option<ConversationDetail>, String> {
    let metadata_path = conversation_metadata_path(conversation_id)?;
    let messages_path = conversation_messages_path(conversation_id)?;
    let Some(data) = read_conversation_file(&metadata_path, &messages_path)? else {
        return Ok(None);
    };

    Ok(Some(ConversationDetail {
        conversation_id: data.meta.conversation_id.clone(),
        title: data.meta.title.clone(),
        title_source: data.meta.title_source.clone(),
        lines: data.lines,
    }))
}

// ── Load title generation candidate ───────────────────────────────────────

pub fn load_title_generation_candidate(
    conversation_id: &str,
) -> Result<Option<TitleGenerationCandidate>, String> {
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
}
