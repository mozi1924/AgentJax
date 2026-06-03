//! OpenAI embedding provider.
//!
//! Calls the OpenAI Embeddings API (`POST /v1/embeddings`) using the
//! existing `reqwest` HTTP client. Credentials and endpoint are resolved
//! from the agent's provider config via `provider_key`.

use async_trait::async_trait;
use serde_json::Value;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::config::EmbeddingProviderConfig;

use super::provider::EmbeddingProvider;
use super::types::{Embedding, EmbeddingRequest, EmbeddingResponse, EmbeddingUsage};

/// OpenAI embedding provider.
pub struct OpenAiEmbeddingProvider {
    model: String,
    dimensions: usize,
    api_endpoint: String,
    credential: Option<String>,
}

impl OpenAiEmbeddingProvider {
    /// Create a new OpenAI embedding provider from an [`EmbeddingProviderConfig`].
    ///
    /// The `provider_key` in the config is used to look up the full provider
    /// config from the app configuration for credential and endpoint resolution.
    pub fn new(config: &EmbeddingProviderConfig) -> Self {
        // Resolve endpoint and credential from the referenced provider config
        let (api_endpoint, credential) = resolve_credentials(config);

        Self {
            model: config.model.clone(),
            dimensions: config.dimensions,
            api_endpoint,
            credential,
        }
    }

    fn build_request_body(&self, input: &EmbeddingRequest) -> Value {
        let model = input.model.as_deref().unwrap_or(&self.model);
        let mut body = serde_json::json!({
            "model": model,
            "input": input.input,
        });
        if let Some(dims) = input.dimensions {
            body["dimensions"] = serde_json::json!(dims);
        }
        body
    }

    fn parse_response(&self, response_body: &Value) -> AgentJaxResult<EmbeddingResponse> {
        let data = response_body["data"]
            .as_array()
            .ok_or_else(|| {
                AgentJaxError::embedding("OpenAI response missing 'data' array")
                    .with_context("parse embedding response")
            })?;

        // Sort by index to maintain input order
        let mut sorted = data.clone();
        sorted.sort_by(|a, b| {
            let ai = a["index"].as_u64().unwrap_or(0);
            let bi = b["index"].as_u64().unwrap_or(0);
            ai.cmp(&bi)
        });

        let embeddings: Vec<Embedding> = sorted
            .iter()
            .map(|entry| {
                entry["embedding"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_f64().map(|f| f as f32))
                            .collect()
                    })
                    .unwrap_or_default()
            })
            .collect();

        let model = response_body["model"]
            .as_str()
            .unwrap_or(&self.model)
            .to_string();

        let usage = response_body["usage"].as_object().map(|u| EmbeddingUsage {
            prompt_tokens: u.get("prompt_tokens").and_then(Value::as_u64).map(|v| v as u32),
            total_tokens: u.get("total_tokens").and_then(Value::as_u64).map(|v| v as u32),
        }).unwrap_or_default();

        Ok(EmbeddingResponse {
            embeddings,
            model,
            usage,
        })
    }
}

#[async_trait]
impl EmbeddingProvider for OpenAiEmbeddingProvider {
    fn provider_name(&self) -> &str {
        "openai"
    }

    fn model_name(&self) -> &str {
        &self.model
    }

    fn dimensions(&self) -> usize {
        self.dimensions
    }

    async fn embed(&self, input: &EmbeddingRequest) -> AgentJaxResult<EmbeddingResponse> {
        let url = format!("{}/v1/embeddings", self.api_endpoint.trim_end_matches('/'));
        let body = self.build_request_body(input);

        let mut req = reqwest::Client::new()
            .post(&url)
            .json(&body);

        if let Some(ref credential) = self.credential {
            req = req.header("Authorization", format!("Bearer {}", credential));
        }

        let resp = req
            .send()
            .await
            .map_err(|e| {
                AgentJaxError::embedding_retryable(format!("OpenAI embedding request failed: {e}"))
                    .with_error_source(&e)
            })?;

        let status = resp.status();
        let response_body: Value = resp
            .json()
            .await
            .map_err(|e| {
                AgentJaxError::embedding(format!("Failed to parse OpenAI embedding response: {e}"))
                    .with_error_source(&e)
            })?;

        if !status.is_success() {
            let error_msg = response_body["error"]["message"]
                .as_str()
                .unwrap_or("unknown error")
                .to_string();
            return Err(match status.as_u16() {
                401 | 403 => AgentJaxError::embedding(format!("OpenAI auth failed: {error_msg}")),
                429 => AgentJaxError::embedding_retryable(format!("OpenAI rate limited: {error_msg}")),
                s if s >= 500 => AgentJaxError::embedding_retryable(format!("OpenAI server error {s}: {error_msg}")),
                _ => AgentJaxError::embedding(format!("OpenAI error {status}: {error_msg}")),
            });
        }

        self.parse_response(&response_body)
    }
}

/// Resolve the API endpoint and credential from the embedding config.
///
/// If a `provider_key` is set, it reads the endpoint and credential from that
/// provider's config in the global app config. Otherwise it falls back to the
/// default OpenAI endpoint.
fn resolve_credentials(config: &EmbeddingProviderConfig) -> (String, Option<String>) {
    // Default fallback
    let default_endpoint = "https://api.openai.com".to_string();

    let provider_key = match config.provider_key.as_deref() {
        Some(key) if !key.is_empty() => key,
        _ => return (default_endpoint, std::env::var("OPENAI_API_KEY").ok()),
    };

    // Try to read the referenced provider config from the app config
    match crate::config::load_config() {
        Ok(app_cfg) => {
            if let Some(provider_cfg) = app_cfg.providers.get(provider_key) {
                let endpoint = provider_cfg.api_endpoint();
                let endpoint = if endpoint.is_empty() {
                    default_endpoint
                } else {
                    endpoint
                };

                let credential = provider_cfg.credential().or_else(|| {
                    let env_var = provider_cfg.credential_env();
                    if env_var.is_empty() {
                        std::env::var("OPENAI_API_KEY").ok()
                    } else {
                        std::env::var(&env_var).ok()
                    }
                });

                (endpoint, credential)
            } else {
                log::warn!(
                    "Embedding config references provider_key '{}' but no such provider config exists",
                    provider_key
                );
                (default_endpoint, std::env::var("OPENAI_API_KEY").ok())
            }
        }
        Err(e) => {
            log::warn!("Failed to load config for embedding credentials: {e}");
            (default_endpoint, std::env::var("OPENAI_API_KEY").ok())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::EmbeddingProviderConfig;
    use serde_json::json;

    #[test]
    fn test_build_request_body_defaults() {
        let config = EmbeddingProviderConfig {
            provider: "openai".to_string(),
            provider_key: None,
            model: "text-embedding-3-small".to_string(),
            dimensions: 1536,
        };
        let provider = OpenAiEmbeddingProvider::new(&config);

        let req = EmbeddingRequest::single("Hello world");
        let body = provider.build_request_body(&req);

        assert_eq!(body["model"], "text-embedding-3-small");
        assert_eq!(body["input"], json!(["Hello world"]));
        assert!(body.get("dimensions").is_none());
    }

    #[test]
    fn test_build_request_body_with_dimensions() {
        let config = EmbeddingProviderConfig::default();
        let provider = OpenAiEmbeddingProvider::new(&config);

        let req = EmbeddingRequest::single("test").with_dimensions(256);
        let body = provider.build_request_body(&req);

        assert_eq!(body["dimensions"], 256);
    }

    #[test]
    fn test_parse_response() {
        let config = EmbeddingProviderConfig::default();
        let provider = OpenAiEmbeddingProvider::new(&config);

        let response = json!({
            "data": [
                {"index": 1, "embedding": [0.3, 0.4]},
                {"index": 0, "embedding": [0.1, 0.2]}
            ],
            "model": "text-embedding-3-small",
            "usage": {"prompt_tokens": 4, "total_tokens": 4}
        });

        let parsed = provider.parse_response(&response).unwrap();
        assert_eq!(parsed.embeddings.len(), 2);
        // Should be sorted by index: [0.1, 0.2] first, then [0.3, 0.4]
        assert!((parsed.embeddings[0][0] - 0.1).abs() < 1e-6);
        assert!((parsed.embeddings[1][0] - 0.3).abs() < 1e-6);
        assert_eq!(parsed.model, "text-embedding-3-small");
        assert_eq!(parsed.usage.prompt_tokens, Some(4));
    }
}
