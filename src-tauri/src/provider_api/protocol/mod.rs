//! Protocol implementations for standard API formats.
//!
//! Each protocol is a self-contained Rust implementation of a standard API
//! format (Responses, Chat Completions, Embeddings). Provider plugins declare
//! which protocols they support; the framework routes requests accordingly.

pub(crate) mod responses;
pub(crate) mod chat;
pub(crate) mod embeddings;

use std::time::Duration;

use crate::config::{AppConfig, ProviderConfig};
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::error_classifier::{classify_http_error, classify_reqwest_error};
use crate::provider_api::capabilities::ProviderCapabilities;
use crate::provider_api::network::apply_headers_to_reqwest;
use crate::provider_api::types::{
    EmbeddingRequest, EmbeddingResponse, ProviderModelDescriptor, ProviderStreamEvent,
    ResponseStreamRequest, ResponseStreamResult,
};
use serde_json::Value;
use tokio::sync::watch;

// ── Circuit Breaker ──────────────────────────────────────────────────────────

static CIRCUIT_BREAKERS: std::sync::LazyLock<
    crate::provider_api::circuit_breaker::CircuitBreakerRegistry,
> = std::sync::LazyLock::new(
    crate::provider_api::circuit_breaker::CircuitBreakerRegistry::new,
);

// ── Streaming Dispatch ──────────────────────────────────────────────────────

/// Stream a response using a native protocol implementation.
///
/// `F` must be `Send` because the future may cross `tokio::spawn` boundaries.
pub async fn stream_response<F>(
    protocol: &str,
    config: &AppConfig,
    provider_key: &str,
    provider_config: &ProviderConfig,
    model_id: &str,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: F,
) -> AgentJaxResult<ResponseStreamResult>
where
    F: FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send,
{
    CIRCUIT_BREAKERS.check(provider_key)?;

    let result = match protocol {
        "responses" => {
            responses::stream_response(
                config, provider_key, provider_config, model_id,
                req, cancel_rx, on_delta,
            )
            .await
        }
        "chat_completions" => {
            chat::stream_response(
                config, provider_key, provider_config, model_id,
                req, cancel_rx, on_delta,
            )
            .await
        }
        other => Err(AgentJaxError::config(format!(
            "Unsupported protocol '{other}'. Supported: responses, chat_completions, embeddings"
        ))),
    };

    match &result {
        Ok(_) => CIRCUIT_BREAKERS.record_success(provider_key),
        Err(err) => {
            if err.kind.is_retryable() {
                CIRCUIT_BREAKERS.record_failure(provider_key);
            }
        }
    }

    result
}

// ── Embedding Dispatch ──────────────────────────────────────────────────────

/// Embed text using a native protocol implementation.
pub async fn embed(
    protocol: &str,
    provider_config: &ProviderConfig,
    model_id: &str,
    input: &EmbeddingRequest,
) -> AgentJaxResult<EmbeddingResponse> {
    match protocol {
        "embeddings" => embeddings::embed(provider_config, model_id, input).await,
        other => Err(AgentJaxError::config(format!(
            "Unsupported protocol '{other}' for embedding"
        ))),
    }
}

// ── Model Fetching ──────────────────────────────────────────────────────────

/// Fetch the remote model list using HTTP GET.
pub async fn fetch_remote_models(
    provider_config: &ProviderConfig,
    endpoint: &str,
    timeout_seconds: u64,
) -> AgentJaxResult<Vec<ProviderModelDescriptor>> {
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|err| AgentJaxError::network(format!("Failed to init HTTP client: {err}")))?;

    let credential = provider_config.resolved_credential();
    let mut builder = client.get(endpoint);
    if let Some(ref credential) = credential {
        builder = builder.header("Authorization", format!("Bearer {credential}"));
    }

    let headers = provider_config.resolved_http_headers();
    builder = apply_headers_to_reqwest(builder, &headers)?;

    let response = builder
        .send()
        .await
        .map_err(|err| classify_reqwest_error(&err, Some(&provider_config.kind)))?;

    if !response.status().is_success() {
        let status = response.status();
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(classify_http_error(
            status.as_u16(),
            &text,
            Some(&provider_config.kind),
            None,
        ));
    }

    let body: Value = response
        .json()
        .await
        .map_err(|err| AgentJaxError::internal(format!("Failed to parse models response: {err}")))?;

    let models = body
        .get("data")
        .or_else(|| body.get("models"))
        .and_then(Value::as_array)
        .map(|arr| {
            arr.iter()
                .filter_map(|model| {
                    let id = model
                        .get("id")
                        .or_else(|| model.get("name"))
                        .and_then(Value::as_str)?;
                    let levels = model
                        .get("supported_reasoning_levels")
                        .or_else(|| model.get("supportedReasoningLevels"))
                        .and_then(Value::as_array)
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect()
                        })
                        .unwrap_or_default();
                    Some(ProviderModelDescriptor {
                        id: id.to_string(),
                        supported_reasoning_levels: levels,
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    Ok(models)
}

// ── Shared HTTP Helpers ─────────────────────────────────────────────────────

pub(crate) fn build_client(timeout_seconds: u64) -> AgentJaxResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_seconds))
        .build()
        .map_err(|err| AgentJaxError::network(format!("Failed to init HTTP client: {err}")))
}

pub(crate) async fn send_and_check(
    builder: reqwest::RequestBuilder,
    provider_kind: &str,
) -> AgentJaxResult<reqwest::Response> {
    let response = builder
        .send()
        .await
        .map_err(|err| classify_reqwest_error(&err, Some(provider_kind)))?;

    if !response.status().is_success() {
        let status = response.status();
        let retry_after = response
            .headers()
            .get("retry-after")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.parse::<u64>().ok())
            .map(Duration::from_secs);
        let text = response
            .text()
            .await
            .unwrap_or_else(|_| "<unable to read error body>".to_string());
        return Err(classify_http_error(
            status.as_u16(),
            &text,
            Some(provider_kind),
            retry_after,
        ));
    }

    Ok(response)
}

/// Protocol-level capabilities.
pub fn protocol_capabilities(protocol: &str) -> Option<ProviderCapabilities> {
    match protocol {
        "responses" => Some(ProviderCapabilities::openai_responses()),
        "chat_completions" => Some(ProviderCapabilities::chat_completions()),
        _ => None,
    }
}
