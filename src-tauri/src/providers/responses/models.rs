use serde_json::Value;
use std::time::Duration;
use tokio::time::sleep;

use crate::config::ResolvedModelConfig;
use crate::providers::types::ProviderModelDescriptor;

use super::{http, normalize_reasoning_levels};

pub struct ModelsFetchStrategy {
    pub endpoint_candidates: Vec<String>,
    pub credential_query_param: Option<&'static str>,
}

impl ModelsFetchStrategy {
    pub fn openai_compatible() -> Self {
        Self {
            endpoint_candidates: vec!["/models".to_string(), "models".to_string()],
            credential_query_param: None,
        }
    }

    pub fn with_provider_overrides(self, overrides: &[String]) -> Self {
        if overrides.is_empty() {
            return self;
        }

        let mut merged = Vec::new();
        for candidate in overrides {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !merged.iter().any(|existing| existing == trimmed) {
                merged.push(trimmed.to_string());
            }
        }

        for candidate in &self.endpoint_candidates {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                continue;
            }
            if !merged.iter().any(|existing| existing == trimmed) {
                merged.push(trimmed.to_string());
            }
        }

        Self {
            endpoint_candidates: merged,
            credential_query_param: self.credential_query_param,
        }
    }

    pub fn with_credential_query_param(mut self, param_name: &'static str) -> Self {
        self.credential_query_param = Some(param_name);
        self
    }
}

pub async fn fetch_remote_models_with_strategy(
    resolved: &ResolvedModelConfig,
    strategy: &ModelsFetchStrategy,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let credential = resolved.provider.resolved_credential();
    let header_credential = strategy
        .credential_query_param
        .is_none()
        .then_some(credential.as_deref())
        .flatten();
    let request_headers =
        http::merge_request_headers(&[], &resolved.provider, None, header_credential);

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;
    let max_retries = resolved.provider.request_max_retries.unwrap_or(0);

    let mut errors = Vec::new();
    for candidate in &strategy.endpoint_candidates {
        let endpoint = build_models_endpoint(&resolved.provider.api_endpoint, candidate);
        let mut query_params = resolved.provider.query_params.clone();
        if let Some(param_name) = strategy.credential_query_param {
            if let Some(credential) = credential
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                query_params
                    .entry(param_name.to_string())
                    .or_insert_with(|| credential.to_string());
            }
        }
        let endpoint = match http::apply_query_params_to_url(&endpoint, &query_params) {
            Ok(endpoint) => endpoint,
            Err(err) => {
                errors.push(format!("{endpoint}: failed to apply query params: {err}"));
                continue;
            }
        };
        let mut attempt = 0u32;
        loop {
            let request = match http::apply_headers_to_reqwest(
                client.get(endpoint.clone()),
                &request_headers,
            ) {
                Ok(request) => request,
                Err(err) => {
                    errors.push(format!(
                        "{endpoint}: failed to apply request headers: {err}"
                    ));
                    break;
                }
            };
            let response = match request.send().await {
                Ok(response) => response,
                Err(err) => {
                    if attempt < max_retries {
                        attempt += 1;
                        sleep(retry_delay(attempt)).await;
                        continue;
                    }
                    errors.push(format!("{endpoint}: request failed: {err}"));
                    break;
                }
            };

            if !response.status().is_success() {
                let status = response.status();
                let text = response
                    .text()
                    .await
                    .unwrap_or_else(|_| "<unable to read error body>".to_string());

                if should_retry_http_status(status.as_u16()) && attempt < max_retries {
                    attempt += 1;
                    sleep(retry_delay(attempt)).await;
                    continue;
                }

                errors.push(format!("{endpoint}: http {status}: {text}"));
                break;
            }

            let body: Value = response
                .json()
                .await
                .map_err(|e| format!("Failed to parse remote model list JSON: {e}"))?;

            let models = parse_model_descriptors(&body);
            if !models.is_empty() {
                return Ok(models);
            }

            // Endpoint is valid but has no models in expected shape; keep trying fallbacks.
            errors.push(format!(
                "{endpoint}: response did not contain recognized model list fields"
            ));
            break;
        }
    }

    Err(format!(
        "Failed to fetch remote models for provider '{}': {}",
        resolved.provider_key,
        errors.join(" | ")
    ))
}

fn should_retry_http_status(status: u16) -> bool {
    matches!(status, 408 | 425 | 429 | 500 | 502 | 503 | 504)
}

fn retry_delay(attempt: u32) -> Duration {
    let shift = attempt.saturating_sub(1).min(4);
    let multiplier = 1u64 << shift;
    Duration::from_millis((150 * multiplier).min(2400))
}

fn build_models_endpoint(base_endpoint: &str, candidate: &str) -> String {
    if candidate.starts_with("http://") || candidate.starts_with("https://") {
        return candidate.to_string();
    }

    let base = base_endpoint.trim_end_matches('/');
    let path = candidate.trim();
    if path.is_empty() {
        return format!("{base}/models");
    }

    let normalized_path = path.trim_start_matches('/');
    format!("{base}/{normalized_path}")
}

fn parse_model_descriptors(root: &Value) -> Vec<ProviderModelDescriptor> {
    let mut out = Vec::new();

    let mut append_items = |items: &[Value]| {
        for item in items {
            if let Some(descriptor) = parse_model_descriptor(item) {
                out.push(descriptor);
            }
        }
    };

    if let Some(items) = root.get("data").and_then(Value::as_array) {
        append_items(items);
    }
    if let Some(items) = root.get("models").and_then(Value::as_array) {
        append_items(items);
    }
    if let Some(items) = root.get("results").and_then(Value::as_array) {
        append_items(items);
    }
    if let Some(items) = root.as_array() {
        append_items(items);
    }

    out
}

fn parse_model_descriptor(item: &Value) -> Option<ProviderModelDescriptor> {
    match item {
        Value::String(id) => {
            let id = id.trim();
            if id.is_empty() {
                return None;
            }
            Some(ProviderModelDescriptor {
                id: id.to_string(),
                supported_reasoning_levels: Vec::new(),
            })
        }
        Value::Object(obj) => {
            let raw_id = obj
                .get("id")
                .and_then(Value::as_str)
                .or_else(|| obj.get("slug").and_then(Value::as_str))
                .or_else(|| obj.get("name").and_then(Value::as_str))
                .or_else(|| obj.get("model").and_then(Value::as_str))
                .unwrap_or("")
                .trim();
            if raw_id.is_empty() {
                return None;
            }

            let supported_reasoning_levels =
                parse_supported_reasoning_levels(obj).unwrap_or_default();

            Some(ProviderModelDescriptor {
                id: raw_id.to_string(),
                supported_reasoning_levels,
            })
        }
        _ => None,
    }
}

fn parse_supported_reasoning_levels(obj: &serde_json::Map<String, Value>) -> Option<Vec<String>> {
    let levels_raw = obj
        .get("supportedReasoningLevels")
        .or_else(|| obj.get("supported_reasoning_levels"))?;
    let arr = levels_raw.as_array()?;

    let mut levels = Vec::new();
    for item in arr {
        if let Some(level) = item.as_str() {
            levels.push(level.to_string());
            continue;
        }

        if let Some(level_obj) = item.as_object() {
            if let Some(level) = level_obj
                .get("effort")
                .and_then(Value::as_str)
                .or_else(|| level_obj.get("level").and_then(Value::as_str))
                .or_else(|| level_obj.get("name").and_then(Value::as_str))
            {
                levels.push(level.to_string());
            }
        }
    }

    Some(normalize_reasoning_levels(&levels))
}

pub(crate) fn infer_reasoning_levels_from_model_id(model_id: &str) -> Vec<String> {
    let model = model_id.trim().to_lowercase();
    if model.is_empty() {
        return Vec::new();
    }

    if model.starts_with("gpt-5-pro") {
        return vec!["high".to_string()];
    }

    if model.starts_with("gpt-5.2-pro") {
        return vec![
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    if model.starts_with("gpt-5.2-codex") {
        return vec![
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    if model.starts_with("gpt-5.2") {
        return vec![
            "none".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
            "xhigh".to_string(),
        ];
    }

    if model.starts_with("gpt-5.1") {
        return vec![
            "none".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ];
    }

    if model.starts_with("gpt-5") {
        return vec![
            "minimal".to_string(),
            "low".to_string(),
            "medium".to_string(),
            "high".to_string(),
        ];
    }

    if model.starts_with("o1") || model.starts_with("o3") || model.starts_with("o4") {
        return vec!["low".to_string(), "medium".to_string(), "high".to_string()];
    }

    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::{ModelsFetchStrategy, parse_model_descriptors};
    use serde_json::json;

    #[test]
    fn parses_openai_data_shape() {
        let body = json!({
            "data": [
                { "id": "gpt-5-mini", "supportedReasoningLevels": ["low", "high"] }
            ]
        });

        let models = parse_model_descriptors(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "gpt-5-mini");
        assert_eq!(models[0].supported_reasoning_levels, vec!["low", "high"]);
    }

    #[test]
    fn parses_gemini_like_models_shape() {
        let body = json!({
            "models": [
                { "name": "models/gemini-2.5-pro" }
            ]
        });

        let models = parse_model_descriptors(&body);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "models/gemini-2.5-pro");
    }

    #[test]
    fn parses_codex_models_shape_with_reasoning_effort_objects() {
        let body = json!({
            "data": [
                { "id": "gpt-5.2-codex" }
            ],
            "models": [
                {
                    "slug": "gpt-5.2-codex",
                    "supported_reasoning_levels": [
                        { "effort": "low" },
                        { "effort": "medium" },
                        { "effort": "high" },
                        { "effort": "xhigh" }
                    ]
                }
            ]
        });

        let models = parse_model_descriptors(&body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[1].id, "gpt-5.2-codex");
        assert_eq!(
            models[1].supported_reasoning_levels,
            vec!["low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn parses_root_array_shape() {
        let body = json!([
            { "id": "model-a" },
            "model-b"
        ]);

        let models = parse_model_descriptors(&body);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "model-a");
        assert_eq!(models[1].id, "model-b");
    }

    #[test]
    fn strategy_prefers_overrides_and_keeps_defaults() {
        let strategy = ModelsFetchStrategy::openai_compatible().with_provider_overrides(&[
            "/custom-models".to_string(),
            " /custom-models ".to_string(),
        ]);

        assert_eq!(strategy.endpoint_candidates[0], "/custom-models");
        assert!(
            strategy
                .endpoint_candidates
                .iter()
                .any(|candidate| candidate == "/models")
        );
    }
}
