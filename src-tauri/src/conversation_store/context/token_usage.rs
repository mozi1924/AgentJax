use serde_json::Value;

mod messages;
mod multimodal;
mod tokenizer;
mod types;

use messages::{build_chat_completion_messages, build_system_input_item};
use tokenizer::{count_model_tokens, count_serialized_tool_schema_tokens};
pub use types::{ConversationTokenUsage, TokenCountFunctionCall, TokenCountMessage};

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
) -> crate::error::AgentJaxResult<ConversationTokenUsage> {
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

/// Count the token usage for a conversation's persisted history (test helper).
#[cfg(test)]
pub fn count_conversation_context_tokens(
    model: &str,
    lines: &[crate::conversation_store::ConversationLine],
) -> crate::error::AgentJaxResult<ConversationTokenUsage> {
    let mut items = super::builders::build_context_items(lines);
    items = super::sanitizer::sanitize_tool_call_pairs(items);
    items = super::truncation::truncate_context_items_preserving_tool_pairs(items, super::policy::MAX_CONTEXT_ITEMS_PER_REQUEST);
    count_request_prompt_tokens(model, None, &items, &[])
}

/// Count the token usage for the conversation snapshot plus prompt composer
/// pieces that will be prepended at runtime.
///
/// This is the UI-facing helper for "what will the next request roughly cost"
/// and includes:
/// - the resolved system prompt
/// - active system prompt blocks
/// - the optional recovery note
/// - the persisted conversation history
pub fn count_conversation_prompt_tokens(
    model: &str,
    instructions_text: Option<&str>,
    system_items: &[Value],
    recovery_note: Option<&Value>,
    context_items: &[Value],
    extra_input_items: &[Value],
    tools: &[Value],
) -> crate::error::AgentJaxResult<ConversationTokenUsage> {
    let mut items = Vec::with_capacity(
        system_items
            .len()
            .saturating_add(recovery_note.map(|_| 1).unwrap_or(0))
            .saturating_add(context_items.len())
            .saturating_add(extra_input_items.len()),
    );
    items.extend(system_items.iter().cloned());
    if let Some(recovery_note) = recovery_note {
        items.push(recovery_note.clone());
    }
    items.extend(context_items.iter().cloned());
    items.extend(extra_input_items.iter().cloned());

    count_request_prompt_tokens(model, instructions_text, &items, tools)
}

/// Count token usage for already-built chat messages.
pub fn count_messages_tokens(model: &str, messages: &[TokenCountMessage]) -> crate::error::AgentJaxResult<usize> {
    if messages.is_empty() {
        return Ok(0);
    }

    let mut total = 3usize; // Assistant priming overhead used by chat-style APIs.
    for message in messages {
        total = total.saturating_add(3); // role/content wrapper overhead
        total = total.saturating_add(count_model_tokens(model, &message.role));
        if let Some(content) = message.content.as_deref() {
            total = total.saturating_add(count_model_tokens(model, content));
        }
        if let Some(name) = message.name.as_deref() {
            total = total.saturating_add(1);
            total = total.saturating_add(count_model_tokens(model, name));
        }
        if let Some(function_call) = &message.function_call {
            total = total.saturating_add(count_model_tokens(model, &function_call.name));
            total = total.saturating_add(count_model_tokens(model, &function_call.arguments));
        }
        total = total.saturating_add(message.multimodal_tokens);
    }
    Ok(total)
}

/// Count the tokenizer token count for a plain text string for the given model.
///
/// This is a lightweight helper exposed for the streaming event path so the
/// backend can report the live token count without reloading the full
/// conversation context on every event.
pub fn count_text_tokens(model: &str, text: &str) -> crate::error::AgentJaxResult<usize> {
    if text.is_empty() {
        return Ok(0);
    }
    Ok(count_model_tokens(model, text))
}

/// Count token usage for serialized tool schemas.
pub fn count_tool_schema_tokens(model: &str, tools: &[Value]) -> crate::error::AgentJaxResult<usize> {
    if tools.is_empty() {
        return Ok(0);
    }

    count_serialized_tool_schema_tokens(model, tools)
}

#[cfg(test)]
mod tests {
    use super::{
        TokenCountFunctionCall, TokenCountMessage, count_conversation_context_tokens,
        count_conversation_prompt_tokens, count_messages_tokens, count_request_prompt_tokens,
        count_tool_schema_tokens,
    };
    use crate::conversation_store::{
        AssistantLine, AssistantStatus, ConversationLine, ToolLine, ToolStatus, UserLine,
    };
    use serde_json::json;

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
                thinking: None,
                thinking_token_count: None,
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
            count_conversation_context_tokens("openai/gpt-5-mini", &lines).unwrap();
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
    fn counts_system_prompt_items() {
        let system_items = vec![json!({
            "role": "system",
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
            "openai/gpt-5-mini",
            Some("You are a helpful assistant."),
            &system_items,
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
            TokenCountMessage {
                role: "assistant".to_string(),
                content: None,
                name: None,
                function_call: Some(TokenCountFunctionCall {
                    name: "lookup".to_string(),
                    arguments: "{\"query\":\"alpha\"}".to_string(),
                }),
                multimodal_tokens: 0,
            },
            TokenCountMessage {
                role: "tool".to_string(),
                content: Some("{\"ok\":true}".to_string()),
                name: Some("lookup".to_string()),
                function_call: None,
                multimodal_tokens: 0,
            },
        ];

        let tokens = count_messages_tokens("openai/gpt-5-mini", &messages).unwrap();
        assert!(tokens > 0);
    }

    #[test]
    fn counts_image_parts_with_fixed_multimodal_weight() {
        let input_items = vec![json!({
            "role": "user",
            "content": [{
                "type": "input_image",
                "image_url": {
                    "url": "https://example.test/image.png",
                    "detail": "high",
                    "width": 1024,
                    "height": 512
                }
            }]
        })];

        let usage = count_request_prompt_tokens("gpt-5-mini", None, &input_items, &[]).unwrap();

        assert!(usage.context_tokens >= 425);
        assert_eq!(usage.context_tokens, usage.prompt_tokens);
    }
}
