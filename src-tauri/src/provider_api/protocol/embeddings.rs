//! OpenAI Embeddings API protocol implementation.
//!
//! Implements the `POST /v1/embeddings` protocol natively in Rust.
//! Sends text to the embeddings API and returns vector embeddings.
//!
//! This replaces the old `embeddings/openai.rs` implementation. Unlike the
//! old system which had its own credential resolution and config parsing,
//! this uses the unified provider config from `provider_api`.

use crate::config::ProviderConfig;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::provider_api::network::apply_headers_to_reqwest;
use crate::provider_api::types::{Embedding, EmbeddingRequest, EmbeddingResponse, EmbeddingUsage};
use serde_json::{Value, json};
use std::time::Duration;

/// Embed text using the OpenAI Embeddings API.
///
/// Builds the HTTP request body, sends it to `{apiEndpoint}/v1/embeddings`,
/// and returns the embedding vectors with usage statistics.
pub async fn embed(
    provider_config: &ProviderConfig,
    model_id: &str,
    input: &EmbeddingRequest,
) -> AgentJaxResult<EmbeddingResponse> {
    let base_url = provider_config.api_endpoint().trim_end_matches('/').to_string();
    let url = format!("{}/embeddings", base_url);

    // Build request body
    let effective_model = input.model.as_deref().unwrap_or(model_id);
    let mut body = json!({
        "model": effective_model,
        "input": input.input,
    });
    if let Some(dims) = input.dimensions {
        body["dimensions"] = json!(dims);
    }

    // Create HTTP client
    let timeout_seconds = provider_config.request_timeout_seconds().unwrap_or(60);
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|err| AgentJaxError::network(format!("Failed to initialize HTTP client: {err}")))?;

    let credential = provider_config.resolved_credential();
    let mut builder = client.post(&url).json(&body);

    if let Some(ref credential) = credential {
        builder = builder.header("Authorization", format!("Bearer {}", credential));
    }

    let headers = provider_config.resolved_http_headers();
    builder = apply_headers_to_reqwest(builder, &headers)?;

    let response = builder.send().await
        .map_err(|err| AgentJaxError::embedding_retryable(
            format!("Embedding request failed: {err}")
        ))?;

    let status = response.status();
    let response_body: Value = response.json().await
        .map_err(|err| AgentJaxError::embedding(
            format!("Failed to parse embedding response: {err}")
        ))?;

    if !status.is_success() {
        let error_msg = response_body["error"]["message"]
            .as_str()
            .unwrap_or("unknown error");
        return Err(match status.as_u16() {
            401 | 403 => AgentJaxError::embedding(format!("Auth failed: {error_msg}")),
            429 => AgentJaxError::embedding_retryable(format!("Rate limited: {error_msg}")),
            s if s >= 500 => AgentJaxError::embedding_retryable(format!("Server error {s}: {error_msg}")),
            _ => AgentJaxError::embedding(format!("Error {status}: {error_msg}")),
        });
    }

    parse_embedding_response(&response_body, model_id)
}

/// Parse the OpenAI Embeddings API response into our internal format.
fn parse_embedding_response(response_body: &Value, default_model: &str) -> AgentJaxResult<EmbeddingResponse> {
    let data = response_body["data"]
        .as_array()
        .ok_or_else(|| AgentJaxError::embedding("Response missing 'data' array"))?;

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
        .unwrap_or(default_model)
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
