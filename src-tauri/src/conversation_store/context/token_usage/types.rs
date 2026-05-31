use serde::Serialize;

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
pub(super) struct MessageContentEstimate {
    pub(super) text: String,
    pub(super) multimodal_tokens: usize,
}
