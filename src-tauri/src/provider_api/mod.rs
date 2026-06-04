//! Provider API — unified abstraction for model providers.
//!
//! This module is the central dispatch for all model interactions:
//!
//! - **Native protocol implementations** (`protocol/`) handle standard OpenAI
//!   API formats (Responses, Chat Completions, Embeddings) directly in Rust.
//! - **JS plugin adapters** (`streaming` module, kept for backward compat) handle
//!   non-standard API formats via deno_core.
//! - **Registry** manages provider metadata (capabilities, config schemas, plugins).
//!
//! The dispatch logic in `stream_response` and `embed` first checks if the
//! provider uses a known protocol. If yes, it routes to the native Rust
//! implementation. Otherwise, it falls back to the JS plugin path.

pub mod capabilities;
pub mod circuit_breaker;
pub mod core;
pub mod network;
pub(crate) mod protocol;
pub mod registry;
pub mod retry;
pub(crate) mod plugin;
pub mod types;

use crate::error::{AgentJaxError, AgentJaxResult};
use serde_json::Value;

pub use capabilities::ProviderCapabilities;
pub use types::{
    EmbeddingRequest, EmbeddingResponse,
    ModelReasoningCapability, ProviderModelDescriptor, ProviderModelMetadata,
    ProviderPendingToolCall, ProviderStreamEvent, ResponseStreamRequest,
    ResponseStreamResult,
};

// ── Public API Functions ────────────────────────────────────────────────────

pub fn get_capabilities(provider_kind: &str) -> AgentJaxResult<ProviderCapabilities> {
    registry::provider_capabilities(provider_kind)
        .ok_or_else(|| AgentJaxError::config(format!("Unsupported provider kind '{}'. Register a provider plugin to enable it.", provider_kind)))
}

pub fn get_tool_schema_format(provider_kind: &str) -> AgentJaxResult<crate::tools::ToolSchemaFormat> {
    registry::provider_tool_schema_format(provider_kind)
        .ok_or_else(|| AgentJaxError::config(format!("Unsupported provider kind '{}'. Register a provider plugin to enable it.", provider_kind)))
}

pub fn extract_pending_tool_calls(
    _provider_kind: &str,
    output_items: &[Value],
) -> AgentJaxResult<Vec<ProviderPendingToolCall>> {
    Ok(core::extract_pending_tool_calls_from_output(output_items))
}

pub fn build_tool_result_input_item(
    _provider_kind: &str,
    call_id: &str,
    output: &str,
) -> AgentJaxResult<Value> {
    Ok(core::build_tool_result_input_item(call_id, output))
}

pub fn build_user_input_item(_provider_kind: &str, text: &str) -> AgentJaxResult<Value> {
    Ok(core::build_user_input_item(text))
}

pub fn compose_tool_continuation_input(
    _provider_kind: &str,
    output_items: &[Value],
    tool_results_items: Vec<Value>,
) -> AgentJaxResult<Vec<Value>> {
    Ok(core::compose_tool_continuation_input(
        output_items,
        tool_results_items,
    ))
}

// ── Streaming ──────────────────────────────────────────────────────────────

/// Stream a response from a model provider.
///
/// Dispatches to the appropriate protocol implementation (native Rust) or
/// falls back to the JS plugin path for non-standard provider APIs.
pub async fn stream_response<F>(
    config: &crate::config::AppConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
    on_delta: F,
) -> AgentJaxResult<ResponseStreamResult>
where
    F: FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send,
{
    // Resolve the model profile to get provider config and model info
    let resolved = config.resolve_model_profile(req.model.as_deref())?;

    // Determine which protocol to use
    let protocol = resolve_protocol(&resolved.provider.kind, &resolved.model_id, resolved.api_protocol.as_deref());
    if let Some(ref protocol) = protocol {
        let mut on_delta = on_delta;
        protocol::stream_response(
            protocol.as_str(),
            config,
            &resolved.provider_key,
            &resolved.provider,
            &resolved.model_id,
            req,
            cancel_rx,
            &mut on_delta,
        )
        .await
    } else {
        // Fall back to JS plugin path for non-standard providers
        plugin::stream_response(config, req, cancel_rx, on_delta).await
    }
}

// ── Embedding ──────────────────────────────────────────────────────────────

/// Embed text using a model provider.
///
/// Dispatches to the native protocol implementation for known embedding
/// protocols, or returns an error for unsupported provider kinds.
pub async fn embed_text(
    config: &crate::config::AppConfig,
    provider_key: &str,
    model_id: &str,
    input: &EmbeddingRequest,
) -> AgentJaxResult<EmbeddingResponse> {
    let provider = config.resolved_provider(provider_key)?;
    let protocol = resolve_protocol(&provider.kind, model_id, None);
    match protocol.as_deref() {
        Some("embeddings") => protocol::embed("embeddings", &provider, model_id, input).await  /* protocol already known to be "embeddings" */,
        Some(other) => Err(AgentJaxError::config(format!(
            "Provider '{}' uses protocol '{other}' which does not support embedding",
            provider_key
        ))),
        None => Err(AgentJaxError::config(format!(
            "Provider '{}' does not declare a supported embedding protocol",
            provider_key
        ))),
    }
}

// ── Remote Model Fetching ──────────────────────────────────────────────────

pub async fn fetch_remote_models(
    config: &crate::config::AppConfig,
    provider_key: &str,
) -> AgentJaxResult<Vec<ProviderModelDescriptor>> {
    // Try protocol-based model fetching first
    let provider = config.resolved_provider(provider_key)?;
    let protocol = resolve_protocol(&provider.kind, "", None);
    if protocol.is_some() {
        let endpoint = format!("{}/models", provider.api_endpoint().trim_end_matches('/'));
        return protocol::fetch_remote_models(&provider, &endpoint, config.request_timeout_seconds).await;
    }

    // Fall back to JS plugin path only if a plugin is registered for this kind
    if crate::provider_api::registry::provider_plugin_package(&provider.kind).is_some() {
        plugin::fetch_remote_models(config, provider_key).await
    } else {
        log::debug!(
            "No protocol or plugin for provider '{}' (kind: {}), skip model fetch",
            provider_key,
            provider.kind
        );
        Ok(Vec::new())
    }
}

// ── Provider Metadata ──────────────────────────────────────────────────────

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> AgentJaxResult<ModelReasoningCapability> {
    plugin::get_reasoning_capability(provider_kind, model_id, cached_levels)
}

pub fn get_model_metadata(
    provider_kind: &str,
    model_id: &str,
) -> AgentJaxResult<ProviderModelMetadata> {
    plugin::get_model_metadata(provider_kind, model_id)
}

// ── Internal Helpers ───────────────────────────────────────────────────────

/// Map a provider kind and optional model ID to a known protocol.
///
/// The protocol determines which native Rust implementation to use for
/// the API call. Returns `None` for plugin-based providers without a
/// native protocol implementation.
fn resolve_protocol(
    provider_kind: &str,
    model_id: &str,
    api_protocol_override: Option<&str>,
) -> Option<String> {
    // 1. Explicit per-model apiProtocol takes highest precedence
    if let Some(proto) = api_protocol_override {
        let trimmed = proto.trim();
        if !trimmed.is_empty() {
            return Some(trimmed.to_string().to_lowercase());
        }
    }

    // 2. Check the registry for protocol declarations
    let protocols = crate::provider_api::registry::provider_supports_protocols(provider_kind);
    if protocols.is_empty() {
        return None;
    }

    // 3. If the provider supports multiple protocols, use model ID heuristics
    //    to pick the best one. Known OpenAI-native models default to Responses;
    //    unknown / third-party models default to Chat Completions for maximum
    //    compatibility (DeepSeek, Ollama, vLLM, LM Studio, OpenRouter, etc.).
    if protocols.len() > 1 {
        let normalized = model_id.trim().to_lowercase();
        if normalized.contains("embedding") || normalized.starts_with("text-embedding-") {
            return protocols.iter().find(|p| *p == "embeddings").cloned()
                .or_else(|| protocols.first().cloned());
        }

        // Known OpenAI first-party model prefixes that support the Responses API.
        let has_responses = protocols.iter().any(|p| p == "responses");
        let has_chat = protocols.iter().any(|p| p == "chat_completions");
        if has_responses && has_chat && !normalized.is_empty() {
            let is_openai_native = [
                "gpt-5", "gpt-4", "gpt-3.5",
                "o1-", "o1 ", "o3-", "o3 ", "o4-", "o4 ",
            ].iter().any(|prefix| normalized.starts_with(prefix));
            if !is_openai_native {
                return Some("chat_completions".to_string());
            }
        }
    }

    protocols.first().cloned()
}
