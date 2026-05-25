pub fn parse_model_ref(model_ref: &str) -> Option<(String, String)> {
    let trimmed = model_ref.trim();
    let (provider, model_key) = trimmed.split_once('/')?;
    let provider = provider.trim().to_lowercase();
    let model_key = model_key.trim().to_string();
    if provider.is_empty() || model_key.is_empty() {
        return None;
    }
    Some((provider, model_key))
}

pub fn model_ref(provider_key: &str, model_key: &str) -> String {
    format!("{}/{}", provider_key, model_key)
}
