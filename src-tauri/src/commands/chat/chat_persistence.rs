use super::chat_utils::{now_unix_ms, run_blocking};
use crate::conversation_store;
use crate::providers::types::ResponseStreamResult;
use serde_json::{json, Value};

fn extract_assistant_message_chunks(output_items: &[Value]) -> Vec<String> {
    let mut chunks = Vec::new();

    for item in output_items {
        if item.get("type").and_then(Value::as_str) != Some("message") {
            continue;
        }
        if item.get("role").and_then(Value::as_str) != Some("assistant") {
            continue;
        }

        let Some(content_parts) = item.get("content").and_then(Value::as_array) else {
            continue;
        };

        let mut text = String::new();
        for part in content_parts {
            if part.get("type").and_then(Value::as_str) == Some("output_text") {
                if let Some(t) = part.get("text").and_then(Value::as_str) {
                    text.push_str(t);
                }
            }
        }

        let trimmed = text.trim();
        if !trimmed.is_empty() {
            chunks.push(trimmed.to_string());
        }
    }

    chunks
}

pub fn persist_tool_progress_event(
    conversation_id: &str,
    request_id: &str,
    utility_model: &str,
    event_kind: &str,
    tool_call_id: &str,
    tool_name: Option<&str>,
    payload: Option<&str>,
) -> Result<(), String> {
    if tool_call_id.trim().is_empty() {
        return Ok(());
    }

    let (entry_id, context_item) = match event_kind {
        "tool_call_done" => {
            let name = tool_name
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .unwrap_or("unknown_tool");
            let args = payload.unwrap_or("{}");
            (
                format!("ctx-tool-call-{request_id}-{tool_call_id}"),
                json!({
                    "type": "function_call",
                    "call_id": tool_call_id,
                    "name": name,
                    "arguments": args,
                }),
            )
        }
        "tool_call_exec" => {
            let output = payload.unwrap_or("{}");
            (
                format!("ctx-tool-output-{request_id}-{tool_call_id}"),
                json!({
                    "type": "function_call_output",
                    "call_id": tool_call_id,
                    "output": output,
                }),
            )
        }
        _ => return Ok(()),
    };

    conversation_store::append_context_item(
        conversation_store::AppendContextItemInput {
            conversation_id: conversation_id.to_string(),
            entry_id,
            created_at_unix_ms: now_unix_ms(),
            response_id: None,
            provider: None,
            model_profile: None,
            model_id: None,
            request_id: Some(request_id.to_string()),
            context_item,
            metadata: Default::default(),
        },
        utility_model,
    )
}

pub async fn persist_completed_exchange(
    conversation_id: &str,
    request_id: &str,
    response: &ResponseStreamResult,
    utility_model: &str,
) -> Result<(), String> {
    let conversation_id = conversation_id.to_string();
    let request_id = request_id.to_string();
    let response = response.clone();
    let utility_model = utility_model.to_string();

    run_blocking(move || {
        for (idx, item) in response.output_items.iter().enumerate() {
            conversation_store::append_context_item(
                conversation_store::AppendContextItemInput {
                    conversation_id: conversation_id.clone(),
                    entry_id: format!("ctx-output-item-{request_id}-{idx}"),
                    created_at_unix_ms: now_unix_ms(),
                    response_id: Some(response.response_id.clone()),
                    provider: Some(response.provider_key.clone()),
                    model_profile: Some(response.model_profile.clone()),
                    model_id: Some(response.model_id.clone()),
                    request_id: Some(request_id.clone()),
                    context_item: item.clone(),
                    metadata: Default::default(),
                },
                &utility_model,
            )?;
        }

        let mut chunks = extract_assistant_message_chunks(&response.output_items);
        if chunks.is_empty() && !response.output_text.trim().is_empty() {
            chunks.push(response.output_text.trim().to_string());
        }

        for (idx, chunk) in chunks.iter().enumerate() {
            conversation_store::append_message(
                conversation_store::AppendMessageInput {
                    conversation_id: conversation_id.clone(),
                    entry_id: format!("msg-assistant-{request_id}-{idx}"),
                    role: "assistant".to_string(),
                    text: chunk.clone(),
                    created_at_unix_ms: now_unix_ms(),
                    response_id: Some(response.response_id.clone()),
                    provider: Some(response.provider_key.clone()),
                    model_profile: Some(response.model_profile.clone()),
                    model_id: Some(response.model_id.clone()),
                    request_id: Some(request_id.clone()),
                    context_items: Vec::new(),
                    timeline_events: None,
                    metadata: Default::default(),
                },
                &utility_model,
            )?;
        }

        Ok(())
    })
    .await
}
