use serde_json::Value;
use std::time::Duration;

use crate::config::ResolvedModelConfig;
use crate::providers::types::ProviderModelDescriptor;

use super::normalize_reasoning_levels;

pub struct ModelsFetchStrategy {
    pub endpoint_candidates: Vec<String>,
}

impl ModelsFetchStrategy {
    pub fn openai_compatible() -> Self {
        Self {
            endpoint_candidates: vec!["/models".to_string(), "models".to_string()],
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
        }
    }
}

pub async fn fetch_remote_models_with_strategy(
    resolved: &ResolvedModelConfig,
    strategy: &ModelsFetchStrategy,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let credential = resolved.provider.resolved_credential().ok_or_else(|| {
        format!(
            "Provider '{}' credential is missing.",
            resolved.provider_key
        )
    })?;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(resolved.timeout_seconds))
        .build()
        .map_err(|e| format!("Failed to initialize HTTP client: {e}"))?;

    let mut errors = Vec::new();
    for candidate in &strategy.endpoint_candidates {
        let endpoint = build_models_endpoint(&resolved.provider.api_endpoint, candidate);
        let response = match client
            .get(endpoint.clone())
            .bearer_auth(&credential)
            .send()
            .await
        {
            Ok(response) => response,
            Err(err) => {
                errors.push(format!("{endpoint}: request failed: {err}"));
                continue;
            }
        };

        if !response.status().is_success() {
            let status = response.status();
            let text = response
                .text()
                .await
                .unwrap_or_else(|_| "<unable to read error body>".to_string());
            errors.push(format!("{endpoint}: http {status}: {text}"));
            continue;
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
    }

    Err(format!(
        "Failed to fetch remote models for provider '{}': {}",
        resolved.provider_key,
        errors.join(" | ")
    ))
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
    let candidates = root
        .get("data")
        .and_then(Value::as_array)
        .or_else(|| root.get("models").and_then(Value::as_array))
        .or_else(|| root.get("results").and_then(Value::as_array))
        .or_else(|| root.as_array());

    let Some(items) = candidates else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for item in items {
        match item {
            Value::String(id) => {
                let id = id.trim();
                if !id.is_empty() {
                    out.push(ProviderModelDescriptor {
                        id: id.to_string(),
                        supported_reasoning_levels: Vec::new(),
                    });
                }
            }
            Value::Object(obj) => {
                let raw_id = obj
                    .get("id")
                    .and_then(Value::as_str)
                    .or_else(|| obj.get("name").and_then(Value::as_str))
                    .or_else(|| obj.get("model").and_then(Value::as_str))
                    .unwrap_or("")
                    .trim();

                if raw_id.is_empty() {
                    continue;
                }

                let id = raw_id.to_string();
                let supported_reasoning_levels = obj
                    .get("supportedReasoningLevels")
                    .and_then(Value::as_array)
                    .or_else(|| {
                        obj.get("supported_reasoning_levels")
                            .and_then(Value::as_array)
                    })
                    .map(|arr| {
                        arr.iter()
                            .filter_map(Value::as_str)
                            .map(ToOwned::to_owned)
                            .collect::<Vec<_>>()
                    })
                    .map(|levels| normalize_reasoning_levels(&levels))
                    .unwrap_or_default();

                out.push(ProviderModelDescriptor {
                    id,
                    supported_reasoning_levels,
                });
            }
            _ => {}
        }
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{parse_model_descriptors, ModelsFetchStrategy};
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
        assert!(strategy
            .endpoint_candidates
            .iter()
            .any(|candidate| candidate == "/models"));
    }
}
