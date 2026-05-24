use serde_json::Value;

#[derive(Debug, Clone, Default)]
pub struct ResponseStreamRequest {
    pub input_text: String,
    pub previous_response_id: Option<String>,
    pub model: Option<String>,
    pub context_items: Vec<Value>,
}

#[derive(Debug, Clone)]
pub struct ResponseStreamResult {
    pub response_id: String,
    pub output_text: String,
    pub output_items: Vec<Value>,
    pub provider_key: String,
    pub model_profile: String,
    pub model_id: String,
}
