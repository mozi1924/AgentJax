use serde_json::Value;

/// Model-facing conversation snapshot produced from stored conversation lines.
#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    pub input_items: Vec<Value>,

    /// Estimated token count of the assembled context items (rough estimate,
    /// not exact tokenizer count). Set during context assembly.
    pub estimated_tokens: usize,

    /// Number of tool call entries in the assembled context.
    pub tool_call_count: usize,

    /// Total number of conversation lines loaded from storage.
    pub message_count: usize,
}
