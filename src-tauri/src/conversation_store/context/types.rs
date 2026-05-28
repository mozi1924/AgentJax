use serde_json::Value;

/// Model-facing conversation snapshot produced from stored conversation lines.
#[derive(Debug, Clone, Default)]
pub struct ConversationContext {
    pub input_items: Vec<Value>,
}
