#[derive(Debug, Clone)]
pub struct ProviderMessage {
    pub role: String,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ResponseStreamRequest {
    pub input: String,
    pub continuation_id: Option<String>,
    pub model: Option<String>,
    pub history: Vec<ProviderMessage>,
}

#[derive(Debug, Clone)]
pub struct ResponseStreamResult {
    pub turn_id: String,
    pub output_text: String,
}
