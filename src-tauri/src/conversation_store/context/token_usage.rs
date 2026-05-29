use super::builders::build_context_items;
use super::policy::MAX_CONTEXT_ITEMS_PER_REQUEST;
use super::sanitizer::sanitize_tool_call_pairs;
use super::truncation::truncate_context_items_preserving_tool_pairs;
use crate::conversation_store::ConversationLine;
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};
use tokenizers::Tokenizer;

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

#[derive(Debug, Clone)]
pub struct TokenCountFunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Debug, Clone)]
pub struct TokenCountMessage {
    pub role: String,
    pub content: Option<String>,
    pub name: Option<String>,
    pub function_call: Option<TokenCountFunctionCall>,
    pub multimodal_tokens: usize,
}

#[derive(Debug, Clone, Default)]
struct MessageContentEstimate {
    text: String,
    multimodal_tokens: usize,
}

struct LocalTokenizerManager {
    cache: Mutex<HashMap<String, Arc<Tokenizer>>>,
    failed_loads: Mutex<HashMap<String, String>>,
}

impl LocalTokenizerManager {
    fn new() -> Self {
        Self {
            cache: Mutex::new(HashMap::new()),
            failed_loads: Mutex::new(HashMap::new()),
        }
    }

    fn count_tokens(&self, model: &str, text: &str) -> usize {
        if text.is_empty() {
            return 0;
        }

        let tokenizer = self.get_or_load_tokenizer(model);
        match tokenizer.and_then(|tokenizer| {
            tokenizer
                .encode(text, true)
                .map(|encoding| encoding.get_ids().len())
                .map_err(|err| err.to_string())
        }) {
            Ok(count) => count,
            Err(err) => {
                log::warn!(
                    "Falling back to character-ratio token estimate for model '{}': {}",
                    model,
                    err
                );
                estimate_tokens_from_chars(text)
            }
        }
    }

    fn get_or_load_tokenizer(&self, model: &str) -> Result<Arc<Tokenizer>, String> {
        let tokenizer_id = tokenizer_id_for_model(model)?;
        if let Ok(cache) = self.cache.lock() {
            if let Some(tokenizer) = cache.get(tokenizer_id) {
                return Ok(Arc::clone(tokenizer));
            }
        }
        if let Ok(failed_loads) = self.failed_loads.lock() {
            if let Some(err) = failed_loads.get(tokenizer_id) {
                return Err(err.clone());
            }
        }

        let tokenizer = match Tokenizer::from_pretrained(tokenizer_id, None) {
            Ok(tokenizer) => Arc::new(tokenizer),
            Err(err) => {
                let message = format!("Failed to load tokenizer '{tokenizer_id}': {err}");
                if let Ok(mut failed_loads) = self.failed_loads.lock() {
                    failed_loads.insert(tokenizer_id.to_string(), message.clone());
                }
                return Err(message);
            }
        };

        let mut cache = self
            .cache
            .lock()
            .map_err(|_| "Failed to lock tokenizer cache".to_string())?;
        Ok(Arc::clone(
            cache
                .entry(tokenizer_id.to_string())
                .or_insert_with(|| Arc::clone(&tokenizer)),
        ))
    }
}

fn local_tokenizer_manager() -> &'static LocalTokenizerManager {
    static MANAGER: OnceLock<LocalTokenizerManager> = OnceLock::new();
    MANAGER.get_or_init(LocalTokenizerManager::new)
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
pub fn count_messages_tokens(model: &str, messages: &[TokenCountMessage]) -> Result<usize, String> {
    if messages.is_empty() {
        return Ok(0);
    }

    let manager = local_tokenizer_manager();
    let mut total = 3usize; // Assistant priming overhead used by chat-style APIs.
    for message in messages {
        total = total.saturating_add(3); // role/content wrapper overhead
        total = total.saturating_add(manager.count_tokens(model, &message.role));
        if let Some(content) = message.content.as_deref() {
            total = total.saturating_add(manager.count_tokens(model, content));
        }
        if let Some(name) = message.name.as_deref() {
            total = total.saturating_add(1);
            total = total.saturating_add(manager.count_tokens(model, name));
        }
        if let Some(function_call) = &message.function_call {
            total = total.saturating_add(manager.count_tokens(model, &function_call.name));
            total = total.saturating_add(manager.count_tokens(model, &function_call.arguments));
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
pub fn count_text_tokens(model: &str, text: &str) -> Result<usize, String> {
    if text.is_empty() {
        return Ok(0);
    }
    Ok(local_tokenizer_manager().count_tokens(model, text))
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

    let mut total = 0usize;
    for tool in tools {
        let serialized = serde_json::to_string(tool)
            .map_err(|err| format!("Failed to serialize tool schema for token counting: {err}"))?;
        total = total.saturating_add(local_tokenizer_manager().count_tokens(model, &serialized));
    }
    Ok(total)
}

fn build_chat_completion_messages(items: &[Value]) -> Result<Vec<TokenCountMessage>, String> {
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
                    messages.push(TokenCountMessage {
                        role: "assistant".to_string(),
                        content: None,
                        name: None,
                        function_call: Some(TokenCountFunctionCall { name, arguments }),
                        multimodal_tokens: 0,
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
                messages.push(TokenCountMessage {
                    role: "tool".to_string(),
                    content: Some(output),
                    name,
                    function_call: None,
                    multimodal_tokens: 0,
                });
            }
            "reasoning" => {
                if let Some(summary) = extract_reasoning_summary(item) {
                    messages.push(TokenCountMessage {
                        role: "assistant".to_string(),
                        content: Some(summary),
                        name: None,
                        function_call: None,
                        multimodal_tokens: 0,
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

fn build_message_from_item(item: &Value) -> Option<TokenCountMessage> {
    let role = item.get("role").and_then(Value::as_str)?.trim();
    if role.is_empty() {
        return None;
    }

    let content = extract_message_content(item)?;
    if content.text.trim().is_empty() && content.multimodal_tokens == 0 {
        return None;
    }

    let name = item
        .get("name")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());

    Some(TokenCountMessage {
        role: role.to_string(),
        content: (!content.text.trim().is_empty()).then_some(content.text),
        name,
        function_call: None,
        multimodal_tokens: content.multimodal_tokens,
    })
}

fn extract_message_content(item: &Value) -> Option<MessageContentEstimate> {
    if let Some(content) = item.get("content") {
        if let Some(text) = content.as_str() {
            let trimmed = text.trim();
            return (!trimmed.is_empty()).then(|| MessageContentEstimate {
                text: trimmed.to_string(),
                multimodal_tokens: 0,
            });
        }

        if let Some(parts) = content.as_array() {
            let mut estimate = MessageContentEstimate::default();
            for part in parts {
                if let Some(part_text) = part.get("text").and_then(Value::as_str) {
                    estimate.text.push_str(part_text);
                } else if let Some(part_text) = part.get("input_text").and_then(Value::as_str) {
                    estimate.text.push_str(part_text);
                }

                if is_image_part(part) {
                    estimate.multimodal_tokens = estimate
                        .multimodal_tokens
                        .saturating_add(estimate_image_tokens(part));
                }
            }
            let trimmed = estimate.text.trim();
            if !trimmed.is_empty() || estimate.multimodal_tokens > 0 {
                estimate.text = trimmed.to_string();
                return Some(estimate);
            }
        }
    }

    item.get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(|text| MessageContentEstimate {
            text: text.to_string(),
            multimodal_tokens: 0,
        })
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

fn tokenizer_id_for_model(model: &str) -> Result<&'static str, String> {
    let normalized = normalize_model_name(model).to_ascii_lowercase();
    if normalized.is_empty() {
        return Err("Model name cannot be empty for token counting".to_string());
    }

    if normalized.contains("claude") {
        return Ok("Xenova/claude-3-tokenizer");
    }

    if normalized.contains("gemini") || normalized.contains("gemma") {
        return Ok("google/gemma-2b");
    }

    if normalized.starts_with("gpt-oss") {
        return Ok("openai/gpt-oss-20b");
    }

    // New GPT-5, GPT-4.1, GPT-4o and o-series models map to an o200k-style
    // tokenizer; older GPT-4 / GPT-3.5 style models stay on cl100k-like assets.
    if normalized.starts_with("gpt-3.5")
        || (normalized.starts_with("gpt-4")
            && !normalized.starts_with("gpt-4o")
            && !normalized.starts_with("gpt-4.1")
            && !normalized.starts_with("gpt-4.5"))
    {
        return Ok("Xenova/gpt-4");
    }

    Ok("Xenova/gpt-4o")
}

fn estimate_tokens_from_chars(text: &str) -> usize {
    text.chars().count().saturating_add(3) / 4
}

fn is_image_part(part: &Value) -> bool {
    let part_type = part.get("type").and_then(Value::as_str).unwrap_or("");
    part_type == "image_url"
        || part_type == "input_image"
        || part_type == "image"
        || part.get("image_url").is_some()
}

fn estimate_image_tokens(part: &Value) -> usize {
    let detail = image_detail(part);
    if detail.as_deref() == Some("low") {
        return 85;
    }

    let dimensions = image_dimensions(part);
    if let Some((width, height)) = dimensions {
        let tiles = ceil_div(width, 512).saturating_mul(ceil_div(height, 512));
        return 85usize.saturating_add(tiles.saturating_mul(170));
    }

    // Without dimensions, the safest deterministic fallback is the low-detail
    // floor documented by image-capable OpenAI APIs.
    85
}

fn image_detail(part: &Value) -> Option<String> {
    part.get("detail")
        .and_then(Value::as_str)
        .or_else(|| {
            part.get("image_url")
                .and_then(Value::as_object)
                .and_then(|image| image.get("detail"))
                .and_then(Value::as_str)
        })
        .map(|value| value.to_ascii_lowercase())
}

fn image_dimensions(part: &Value) -> Option<(usize, usize)> {
    let width = part.get("width").and_then(Value::as_u64).or_else(|| {
        part.get("image_url")
            .and_then(Value::as_object)
            .and_then(|image| image.get("width"))
            .and_then(Value::as_u64)
    })? as usize;
    let height = part.get("height").and_then(Value::as_u64).or_else(|| {
        part.get("image_url")
            .and_then(Value::as_object)
            .and_then(|image| image.get("height"))
            .and_then(Value::as_u64)
    })? as usize;
    (width > 0 && height > 0).then_some((width, height))
}

fn ceil_div(value: usize, divisor: usize) -> usize {
    value.saturating_add(divisor.saturating_sub(1)) / divisor
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

        let tokens = count_messages_tokens("openai-responses/gpt-5-mini", &messages).unwrap();
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
