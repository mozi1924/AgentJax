//! Protocol implementations for standard API formats.
//!
//! Each protocol is a self-contained Rust implementation of a standard API
//! format (Responses, Chat Completions, Embeddings). Provider plugins declare
//! which protocols they support; the framework routes requests accordingly.
//!
//! # Protocol Trait & Registry
//!
//! Phase 1 introduces the [`Protocol`] trait and [`ProtocolRegistry`] as a
//! new skeleton alongside the existing free-function dispatch. Protocols are
//! stateless, reusable, and belong to no specific provider. The registry is
//! populated at startup with built-in implementations.
//!
//! See [`builtin_protocols`] for the global registry accessor.

pub(crate) mod base_streaming;
pub(crate) mod chat;
pub(crate) mod embeddings;
pub(crate) mod responses;

use std::collections::HashMap;
use std::sync::OnceLock;
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
> = std::sync::LazyLock::new(crate::provider_api::circuit_breaker::CircuitBreakerRegistry::new);

// ── Streaming Dispatch ──────────────────────────────────────────────────────

/// Stream a response using a native protocol implementation.
///
/// Dispatches to the appropriate [`Protocol`] registered in [`builtin_protocols()`].
/// `F` must be `Send` because the future may cross `tokio::spawn` boundaries.
#[allow(clippy::too_many_arguments)]
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

    let proto = builtin_protocols().get(protocol).ok_or_else(|| {
        AgentJaxError::config(format!(
            "Unsupported protocol '{protocol}'. Supported: {}",
            builtin_protocols().names().collect::<Vec<_>>().join(", ")
        ))
    })?;

    let mut cb = on_delta;
    let result = proto
        .stream_response(
            config,
            provider_key,
            provider_config,
            model_id,
            req,
            cancel_rx,
            &mut cb,
        )
        .await;

    match result {
        Ok(mut res) => {
            // Override capabilities from the protocol trait: the individual
            // protocol functions (chat.rs, responses.rs) may set a reasonable
            // default, but the Protocol trait implementation is the authoritative
            // source.
            res.capabilities = proto.capabilities();
            CIRCUIT_BREAKERS.record_success(provider_key);
            Ok(res)
        }
        Err(err) => {
            if err.kind.is_retryable() {
                CIRCUIT_BREAKERS.record_failure(provider_key);
            }
            Err(err)
        }
    }
}

// ── Embedding Dispatch ──────────────────────────────────────────────────────

/// Embed text using a native protocol implementation.
pub async fn embed(
    protocol: &str,
    provider_config: &ProviderConfig,
    model_id: &str,
    input: &EmbeddingRequest,
) -> AgentJaxResult<EmbeddingResponse> {
    let proto = builtin_protocols().get(protocol).ok_or_else(|| {
        AgentJaxError::config(format!("Unsupported protocol '{protocol}' for embedding"))
    })?;
    proto.embed(provider_config, model_id, input).await
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

    let body: Value = response.json().await.map_err(|err| {
        AgentJaxError::internal(format!("Failed to parse models response: {err}"))
    })?;

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
                        kind: None,
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

// ═════════════════════════════════════════════════════════════════════════════
// Protocol Trait & Registry
// ═════════════════════════════════════════════════════════════════════════════

/// A stateless, reusable protocol implementation.
///
/// Protocols implement a standard API shape (e.g. Chat Completions, Responses,
/// Embeddings) and are not tied to any specific provider. They are registered
/// in the [`ProtocolRegistry`] and dispatched by name.
///
/// All methods are async so implementations can perform HTTP I/O. The trait
/// is `Send + Sync` so protocols can be stored in a global registry.
#[async_trait::async_trait]
pub trait Protocol: Send + Sync {
    /// Canonical protocol name (e.g., `"chat_completions"`, `"responses"`,
    /// `"embeddings"`).
    fn name(&self) -> &str;

    /// Stream a chat completion response from a model via this protocol.
    #[allow(clippy::too_many_arguments)]
    async fn stream_response(
        &self,
        config: &AppConfig,
        provider_key: &str,
        provider_config: &ProviderConfig,
        model_id: &str,
        req: &ResponseStreamRequest,
        cancel_rx: &mut watch::Receiver<bool>,
        on_delta: &mut (dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send + '_),
    ) -> AgentJaxResult<ResponseStreamResult>;

    /// Embed text into vector representations.
    async fn embed(
        &self,
        provider_config: &ProviderConfig,
        model_id: &str,
        input: &EmbeddingRequest,
    ) -> AgentJaxResult<EmbeddingResponse>;

    /// Capabilities advertised by this protocol.
    fn capabilities(&self) -> ProviderCapabilities;
}

// ── Protocol Registry ──────────────────────────────────────────────────────

/// A thread-safe registry of protocol implementations.
///
/// Protocols are registered by name and can be looked up dynamically.
/// Populated at startup with built-in protocol implementations via
/// [`builtin_protocols`]. External registrations may be added in the
/// future for plugin-provided protocols.
pub struct ProtocolRegistry {
    protocols: HashMap<String, Box<dyn Protocol>>,
}

impl ProtocolRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            protocols: HashMap::new(),
        }
    }

    /// Register a protocol implementation.
    ///
    /// If a protocol with the same name is already registered, it is replaced.
    pub fn register(&mut self, protocol: Box<dyn Protocol>) {
        self.protocols.insert(protocol.name().to_string(), protocol);
    }

    /// Look up a protocol by name.
    pub fn get(&self, name: &str) -> Option<&dyn Protocol> {
        self.protocols.get(name).map(|p| p.as_ref())
    }

    /// Return all registered protocol names.
    pub fn names(&self) -> impl Iterator<Item = &str> + '_ {
        self.protocols.keys().map(|s| s.as_str())
    }
}

impl Default for ProtocolRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ── Built-in Protocol Wrappers ─────────────────────────────────────────────
//
// Each wrapper delegates to the existing free-function implementation. This
// keeps old dispatch paths working while establishing the new trait interface.

struct ResponsesProtocol;

#[async_trait::async_trait]
impl Protocol for ResponsesProtocol {
    fn name(&self) -> &str {
        "responses"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::openai_responses()
    }

    async fn stream_response(
        &self,
        config: &AppConfig,
        provider_key: &str,
        provider_config: &ProviderConfig,
        model_id: &str,
        req: &ResponseStreamRequest,
        cancel_rx: &mut watch::Receiver<bool>,
        on_delta: &mut (dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send + '_),
    ) -> AgentJaxResult<ResponseStreamResult> {
        let cb = |event| on_delta(event);
        responses::stream_response(
            config,
            provider_key,
            provider_config,
            model_id,
            req,
            cancel_rx,
            cb,
        )
        .await
    }

    async fn embed(
        &self,
        _provider_config: &ProviderConfig,
        _model_id: &str,
        _input: &EmbeddingRequest,
    ) -> AgentJaxResult<EmbeddingResponse> {
        Err(AgentJaxError::config(
            "Responses protocol does not support embedding",
        ))
    }
}

struct ChatCompletionsProtocol;

#[async_trait::async_trait]
impl Protocol for ChatCompletionsProtocol {
    fn name(&self) -> &str {
        "chat_completions"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::chat_completions()
    }

    async fn stream_response(
        &self,
        config: &AppConfig,
        provider_key: &str,
        provider_config: &ProviderConfig,
        model_id: &str,
        req: &ResponseStreamRequest,
        cancel_rx: &mut watch::Receiver<bool>,
        on_delta: &mut (dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send + '_),
    ) -> AgentJaxResult<ResponseStreamResult> {
        let cb = |event| on_delta(event);
        chat::stream_response(
            config,
            provider_key,
            provider_config,
            model_id,
            req,
            cancel_rx,
            cb,
        )
        .await
    }

    async fn embed(
        &self,
        _provider_config: &ProviderConfig,
        _model_id: &str,
        _input: &EmbeddingRequest,
    ) -> AgentJaxResult<EmbeddingResponse> {
        Err(AgentJaxError::config(
            "Chat Completions protocol does not support embedding",
        ))
    }
}

struct EmbeddingsProtocol;

#[async_trait::async_trait]
impl Protocol for EmbeddingsProtocol {
    fn name(&self) -> &str {
        "embeddings"
    }

    fn capabilities(&self) -> ProviderCapabilities {
        // Embeddings have minimal capabilities — no chat, no tools.
        ProviderCapabilities {
            requires_instructions: false,
            requires_stream_true_in_websocket: false,
            supports_stored_responses: false,
            supports_cross_socket_continuation: false,
            supports_generate_false: false,
            supports_json_mode: false,
            supports_json_schema: false,
            supports_parallel_tool_calls: false,
            supports_built_in_web_search: false,
            emits_final_output_items: false,
            emits_incremental_tool_call_arguments: false,
        }
    }

    async fn stream_response(
        &self,
        _config: &AppConfig,
        _provider_key: &str,
        _provider_config: &ProviderConfig,
        _model_id: &str,
        _req: &ResponseStreamRequest,
        _cancel_rx: &mut watch::Receiver<bool>,
        _on_delta: &mut (dyn FnMut(ProviderStreamEvent) -> AgentJaxResult<()> + Send + '_),
    ) -> AgentJaxResult<ResponseStreamResult> {
        Err(AgentJaxError::config(
            "Embeddings protocol does not support streaming",
        ))
    }

    async fn embed(
        &self,
        provider_config: &ProviderConfig,
        model_id: &str,
        input: &EmbeddingRequest,
    ) -> AgentJaxResult<EmbeddingResponse> {
        embeddings::embed(provider_config, model_id, input).await
    }
}

// ── Global Registry ────────────────────────────────────────────────────────

/// Return the global built-in protocol registry.
///
/// Populated once at first access with all built-in protocol implementations.
/// This is the primary entry point for looking up protocols at runtime.
pub fn builtin_protocols() -> &'static ProtocolRegistry {
    static BUILTIN: OnceLock<ProtocolRegistry> = OnceLock::new();
    BUILTIN.get_or_init(|| {
        let mut registry = ProtocolRegistry::new();
        registry.register(Box::new(ResponsesProtocol));
        registry.register(Box::new(ChatCompletionsProtocol));
        registry.register(Box::new(EmbeddingsProtocol));
        registry
    })
}
