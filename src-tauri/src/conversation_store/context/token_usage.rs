use super::builders::build_context_items;
use super::policy::MAX_CONTEXT_ITEMS_PER_REQUEST;
use super::sanitizer::sanitize_tool_call_pairs;
use super::truncation::truncate_context_items_preserving_tool_pairs;
use crate::conversation_store::ConversationLine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use tiktoken_rs::{
    ChatCompletionRequestMessage, CoreBPE, FunctionCall, cl100k_base_singleton,
    num_tokens_from_messages, o200k_base_singleton, o200k_harmony_singleton,
};

/// Token usage snapshot for the current conversation context.
///
/// `context_tokens` covers the persisted conversation history after the same
/// sanitize/truncate rules used for runtime request assembly.
/// `prompt_tokens` extends that count with any extra request-side payload the
/// caller wants to include, such as tool schemas.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ConversationTokenUsage {
    pub context_tokens: usize,
    pub prompt_tokens: usize,
}

/// Count the token usage for a fully assembled request prompt.
///
/// This accepts the exact model-facing prompt pieces we care about:
/// system instructions, Responses-style input items, and tool schemas.
/// The `context_tokens` field measures the prompt items only, while
/// `prompt_tokens` also includes the serialized tool schemas.
pub fn count_request_prompt_tokens(
    model: &str,
    instructions_text: Option<&str>,
    input_items: &[Value],
    tools: &[Value],
) -> Result<ConversationTokenUsage, String> {
    let mut request_items = Vec::with_capacity(input_items.len().saturating_add(1));
    if let Some(instructions_text) = instructions_text
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        request_items.push(build_system_input_item(instructions_text));
    }
    request_items.extend(input_items.iter().cloned());

    let messages = build_chat_completion_messages(&request_items)?;
    let context_tokens = count_messages_tokens(model, &messages)?;
    let tool_schema_tokens = count_tool_schema_tokens(model, tools)?;

    Ok(ConversationTokenUsage {
        context_tokens,
        prompt_tokens: context_tokens + tool_schema_tokens,
    })
}

/// Count the token usage for a conversation's persisted history.
///
/// This mirrors the same context-building pipeline used before a request is
/// sent so the number shown in the UI stays aligned with the model-facing
/// prompt.
#[allow(dead_code)]
pub fn count_conversation_context_tokens(
    model: &str,
    lines: &[ConversationLine],
) -> Result<ConversationTokenUsage, String> {
    let mut items = build_context_items(lines);
    items = sanitize_tool_call_pairs(items);
    items = truncate_context_items_preserving_tool_pairs(items, MAX_CONTEXT_ITEMS_PER_REQUEST);
    count_request_prompt_tokens(model, None, &items, &[])
}

/// Count the token usage for the conversation snapshot plus prompt composer
/// pieces that will be prepended at runtime.
///
/// This is the UI-facing helper for "what will the next request roughly cost"
/// and includes:
/// - the resolved system prompt
/// - active developer prompt blocks
/// - the optional recovery developer note
/// - the persisted conversation history
pub fn count_conversation_prompt_tokens(
    model: &str,
    instructions_text: Option<&str>,
    developer_items: &[Value],
    recovery_note: Option<&Value>,
    context_items: &[Value],
    extra_input_items: &[Value],
    tools: &[Value],
) -> Result<ConversationTokenUsage, String> {
    let mut items = Vec::with_capacity(
        developer_items
            .len()
            .saturating_add(recovery_note.map(|_| 1).unwrap_or(0))
            .saturating_add(context_items.len())
            .saturating_add(extra_input_items.len()),
    );
    items.extend(developer_items.iter().cloned());
    if let Some(recovery_note) = recovery_note {
        items.push(recovery_note.clone());
    }
    items.extend(context_items.iter().cloned());
    items.extend(extra_input_items.iter().cloned());

    count_request_prompt_tokens(model, instructions_text, &items, tools)
}

/// Count token usage for already-built chat messages.
pub fn count_messages_tokens(
    model: &str,
    messages: &[ChatCompletionRequestMessage],
) -> Result<usize, String> {
    if messages.is_empty() {
        return Ok(0);
    }

    num_tokens_from_messages(normalize_model_name(model).as_str(), messages)
        .map_err(|err| err.to_string())
}

/// Count the tiktoken token count for a plain text string for the given model.
///
/// This is a lightweight helper exposed for the streaming event path so the
/// backend can report the live token count without reloading the full
/// conversation context on every event.
pub fn count_text_tokens(model: &str, text: &str) -> Result<usize, String> {
    if text.is_empty() {
        return Ok(0);
    }
    let bpe = tokenizer_for_model(model)?;
    Ok(bpe.count_with_special_tokens(text))
}

/// Count token usage for serialized tool schemas.
///
/// The schema is tokenized from its compact JSON form so the count reflects
/// the model-facing payload rather than an ad-hoc projection of the object.
#[allow(dead_code)]
pub fn count_tool_schema_tokens(model: &str, tools: &[Value]) -> Result<usize, String> {
    if tools.is_empty() {
        return Ok(0);
    }

    let bpe = tokenizer_for_model(model)?;
    let mut total = 0usize;
    for tool in tools {
        let serialized = serde_json::to_string(tool)
            .map_err(|err| format!("Failed to serialize tool schema for token counting: {err}"))?;
        total = total.saturating_add(bpe.count_with_special_tokens(&serialized));
    }
    Ok(total)
}

fn build_chat_completion_messages(
    items: &[Value],
) -> Result<Vec<ChatCompletionRequestMessage>, String> {
    let mut messages = Vec::new();
    let mut tool_names_by_call_id: HashMap<String, String> = HashMap::new();

    for item in items {
        let item_type = item.get("type").and_then(Value::as_str).unwrap_or("");
        match item_type {
            "message" => {
                if let Some(message) = build_message_from_item(item) {
                    messages.push(message);
                }
            }
            "function_call" | "custom_tool_call" => {
                let call_id = item
                    .get("call_id")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                let arguments = item
                    .get("arguments")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();

                if !call_id.is_empty() && !name.is_empty() {
                    tool_names_by_call_id.insert(call_id, name.clone());
                }

                if !name.is_empty() {
                    messages.push(ChatCompletionRequestMessage {
                        role: "assistant".to_string(),
                        content: None,
                        name: None,
                        function_call: Some(FunctionCall { name, arguments }),
                        tool_calls: Vec::new(),
                        refusal: None,
                    });
                }
            }
            "function_call_output" | "custom_tool_call_output" => {
                let call_id = item.get("call_id").and_then(Value::as_str).unwrap_or("");
                let output = item
                    .get("output")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .to_string();
                if output.trim().is_empty() {
                    continue;
                }

                let name = tool_names_by_call_id.get(call_id).cloned();
                messages.push(ChatCompletionRequestMessage {
                    role: "tool".to_string(),
                    content: Some(output),
                    name,
                    function_call: None,
                    tool_calls: Vec::new(),
                    refusal: None,
                });
            }
            "reasoning" => {
                if let Some(summary) = extract_reasoning_summary(item) {
                    messages.push(ChatCompletionRequestMessage {
                        role: "assistant".to_string(),
                        content: Some(summary),
                        name: None,
                        function_call: None,
                        tool_calls: Vec::new(),
                        refusal: None,
                    });
                }
            }
            _ => {
                if let Some(message) = build_message_from_item(item) {
                    messages.push(message);
                }
            }
        }
    }

    Ok(messages)
}

fn build_system_input_item(text: &str) -> Value {
    json!({
        "role": "system",
        "content": [{
            "type": "input_text",
            "text": text,
        }]
    })
}

fn build_message_from_item(item: &Value) -> Option<ChatCompletionRequestMessage> {
    let role = item.get("role").and_then(Value::as_str)?.trim();
    if role.is_empty() {
        return None;
    }

    let content = extract_message_content(item)?;
    if content.trim().is_empty() {
        return None;
    }

    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Some(ChatCompletionRequestMessage {
        role: role.to_string(),
        content: Some(content),
        name,
        function_call: None,
        tool_calls: Vec::new(),
        refusal: None,
    })
}

fn extract_message_content(item: &Value) -> Option<String> {
    if let Some(content) = item.get("content") {
        if let Some(text) = content.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }

        if let Some(parts) = content.as_array() {
            let mut text = String::new();
            for part in parts {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    text.push_str(part_text);
                }
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    item.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_reasoning_summary(item: &Value) -> Option<String> {
    if let Some(summary) = item.get("summary") {
        if let Some(text) = summary.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| trimmed.to_string());
        }

        if let Some(parts) = summary.as_array() {
            let mut text = String::new();
            for part in parts {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    text.push_str(part_text);
                }
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }

    item.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned)
}

fn normalize_model_name(model: &str) -> String {
    let trimmed = model.trim();
    if let Some((_, model_name)) = trimmed.rsplit_once('/') {
        let model_name = model_name.trim();
        if !model_name.is_empty() {
            return model_name.to_ascii_lowercase();
        }
    }
    trimmed.to_ascii_lowercase()
}

#[allow(dead_code)]
fn tokenizer_for_model(model: &str) -> Result<&'static CoreBPE, String> {
    let normalized = normalize_model_name(model).to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Model name cannot be empty for token counting".to_string());
    }

    // GPT-5, GPT-4.1, GPT-4o and the newer o-series families all use the
    // o200k vocabulary; older GPT-4 / GPT-3.5 style models stay on cl100k.
    if normalized.starts_with("gpt-3.5")
        || (normalized.starts_with("gpt-4")
            && !normalized.starts_with("gpt-4o")
            && !normalized.starts_with("gpt-4.1")
            && !normalized.starts_with("gpt-4.5"))
    {
        return Ok(cl100k_base_singleton());
    }

    if normalized.starts_with("gpt-oss") {
        return Ok(o200k_harmony_singleton());
    }

    Ok(o200k_base_singleton())
}

#[cfg(test)]
mod tests {
    use super::{
        count_conversation_context_tokens, count_conversation_prompt_tokens, count_messages_tokens,
        count_request_prompt_tokens, count_tool_schema_tokens,
    };
    use crate::conversation_store::{
        AssistantLine, AssistantStatus, ConversationLine, ToolLine, ToolStatus, UserLine,
    };
    use serde_json::json;
    use tiktoken_rs::{ChatCompletionRequestMessage, FunctionCall};

    #[test]
    fn counts_conversation_context_tokens_from_lines() {
        let lines = vec![
            ConversationLine::User(UserLine {
                id: "u1".to_string(),
                ts: 1,
                request_id: "req-1".to_string(),
                text: "hello".to_string(),
            }),
            ConversationLine::Assistant(AssistantLine {
                id: "a1".to_string(),
                ts: 2,
                request_id: "req-1".to_string(),
                response_id: "resp-1".to_string(),
                phase: None,
                text: "world".to_string(),
                status: AssistantStatus::Done,
            }),
            ConversationLine::Tool(ToolLine {
                id: "t1".to_string(),
                ts: 3,
                started_ts: 3,
                completed_ts: Some(4),
                request_id: "req-1".to_string(),
                call_id: "call-1".to_string(),
                name: "lookup".to_string(),
                display_name: None,
                description: None,
                icon: None,
                args: json!({"query":"alpha"}),
                output: Some(json!({"ok":true})),
                status: ToolStatus::Done,
            }),
        ];

        let usage =
            count_conversation_context_tokens("openai-responses/gpt-5-mini", &lines).unwrap();
        assert!(usage.context_tokens > 0);
        assert_eq!(usage.context_tokens, usage.prompt_tokens);
    }

    #[test]
    fn counts_messages_and_tools_separately() {
        let input_items = vec![json!({
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "Count me"
            }]
        })];
        let tools = vec![json!({
            "type": "function",
            "function": {
                "name": "lookup",
                "description": "Find a thing",
                "parameters": { "type": "object" }
            }
        })];

        let message_tokens =
            count_request_prompt_tokens("gpt-5-mini", None, &input_items, &[]).unwrap();
        let tool_tokens = count_tool_schema_tokens("gpt-5-mini", &tools).unwrap();
        let usage = count_request_prompt_tokens("gpt-5-mini", None, &input_items, &tools).unwrap();

        assert_eq!(usage.context_tokens, message_tokens.context_tokens);
        assert_eq!(
            usage.prompt_tokens,
            message_tokens.context_tokens + tool_tokens
        );
    }

    #[test]
    fn counts_system_and_developer_prompt_items() {
        let developer_items = vec![json!({
            "role": "developer",
            "content": [{
                "type": "input_text",
                "text": "Always answer in Chinese."
            }]
        })];
        let context_items = vec![json!({
            "role": "user",
            "content": [{
                "type": "input_text",
                "text": "hello"
            }]
        })];

        let usage = count_conversation_prompt_tokens(
            "openai-responses/gpt-5-mini",
            Some("You are a helpful assistant."),
            &developer_items,
            None,
            &context_items,
            &[],
            &[],
        )
        .unwrap();

        assert!(usage.context_tokens > 0);
        assert_eq!(usage.context_tokens, usage.prompt_tokens);
    }

    #[test]
    fn supports_function_call_messages() {
        let messages = vec![
            ChatCompletionRequestMessage {
                role: "assistant".to_string(),
                content: None,
                name: None,
                function_call: Some(FunctionCall {
                    name: "lookup".to_string(),
                    arguments: "{\"query\":\"alpha\"}".to_string(),
                }),
                tool_calls: Vec::new(),
                refusal: None,
            },
            ChatCompletionRequestMessage {
                role: "tool".to_string(),
                content: Some("{\"ok\":true}".to_string()),
                name: Some("lookup".to_string()),
                function_call: None,
                tool_calls: Vec::new(),
                refusal: None,
            },
        ];

        let tokens = count_messages_tokens("openai-responses/gpt-5-mini", &messages).unwrap();
        assert!(tokens > 0);
    }
}
