use std::future::Future;
use std::pin::Pin;

use tokio::sync::watch;

use super::chat_completions;
use super::openai_responses;
use super::types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ResponseStreamRequest,
    ResponseStreamResult,
};
use crate::config::ResolvedModelConfig;

pub(crate) type StreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ResponseStreamResult, String>> + Send + 'a>>;
pub(crate) type ModelsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<ProviderModelDescriptor>, String>> + Send + 'a>>;

/// Adapter boundary between raw provider APIs and AgentJax's normalized runtime
/// protocol. Implementations are responsible for translating provider-specific
/// stream events, tool IDs, and token usage before the data reaches runtime.
pub(crate) trait ProviderAdapter: Send + Sync {
    fn matches_kind(&self, provider_kind: &str) -> bool;
    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a>;
    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a>;
    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability;
}

struct OpenAIResponsesAdapter;

impl ProviderAdapter for OpenAIResponsesAdapter {
    fn matches_kind(&self, provider_kind: &str) -> bool {
        matches!(provider_kind, "openai-responses")
    }

    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a> {
        Box::pin(openai_responses::stream_response(
            resolved, req, cancel_rx, on_delta,
        ))
    }

    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a> {
        Box::pin(openai_responses::fetch_remote_models(resolved))
    }

    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability {
        openai_responses::get_reasoning_capability(model_id, cached_levels)
    }
}

struct ChatCompletionsAdapter;

impl ProviderAdapter for ChatCompletionsAdapter {
    fn matches_kind(&self, provider_kind: &str) -> bool {
        matches!(
            provider_kind,
            "chat-completions" | "openai-chat-completions"
        )
    }

    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a> {
        Box::pin(chat_completions::stream_response(
            resolved, req, cancel_rx, on_delta,
        ))
    }

    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a> {
        Box::pin(chat_completions::fetch_remote_models(resolved))
    }

    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability {
        chat_completions::get_reasoning_capability(model_id, cached_levels)
    }
}

static OPENAI_RESPONSES_ADAPTER: OpenAIResponsesAdapter = OpenAIResponsesAdapter;
static CHAT_COMPLETIONS_ADAPTER: ChatCompletionsAdapter = ChatCompletionsAdapter;

pub(crate) fn adapter_for_kind(
    provider_kind: &str,
) -> Result<&'static dyn ProviderAdapter, String> {
    let normalized = provider_kind.trim().to_lowercase();
    let adapters: [&dyn ProviderAdapter; 2] =
        [&OPENAI_RESPONSES_ADAPTER, &CHAT_COMPLETIONS_ADAPTER];

    for adapter in adapters {
        if adapter.matches_kind(&normalized) {
            return Ok(adapter);
        }
    }

    Err(format!(
        "Unsupported provider kind '{}'. Add an adapter under src-tauri/src/providers to enable it.",
        provider_kind
    ))
}
