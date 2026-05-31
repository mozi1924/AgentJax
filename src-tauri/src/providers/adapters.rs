use std::future::Future;
use std::pin::Pin;

use tokio::sync::watch;

use super::anthropic;
use super::chat_completions;
use super::gemini;
use super::openai_responses;
use super::registry::{self, ProviderTransportFamily};
use super::types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ResponseStreamRequest,
    ResponseStreamResult,
};
use crate::config::ResolvedModelConfig;

pub(crate) type StreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ResponseStreamResult, String>> + Send + 'a>>;
pub(crate) type ModelsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<ProviderModelDescriptor>, String>> + Send + 'a>>;

/// AgentJax provider API boundary between raw provider transports and the
/// normalized runtime protocol. Implementations translate provider-specific
/// stream events, tool IDs, and token usage before the data reaches runtime.
pub(crate) trait AgentJaxProviderApi: Send + Sync {
    fn transport_family(&self) -> ProviderTransportFamily;
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

impl AgentJaxProviderApi for OpenAIResponsesAdapter {
    fn transport_family(&self) -> ProviderTransportFamily {
        ProviderTransportFamily::Responses
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

impl AgentJaxProviderApi for ChatCompletionsAdapter {
    fn transport_family(&self) -> ProviderTransportFamily {
        ProviderTransportFamily::ChatCompletions
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

struct GeminiAdapter;

impl AgentJaxProviderApi for GeminiAdapter {
    fn transport_family(&self) -> ProviderTransportFamily {
        ProviderTransportFamily::Gemini
    }

    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a> {
        Box::pin(gemini::stream_response(resolved, req, cancel_rx, on_delta))
    }

    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a> {
        Box::pin(gemini::fetch_remote_models(resolved))
    }

    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability {
        gemini::get_reasoning_capability(model_id, cached_levels)
    }
}

struct AnthropicAdapter;

impl AgentJaxProviderApi for AnthropicAdapter {
    fn transport_family(&self) -> ProviderTransportFamily {
        ProviderTransportFamily::Anthropic
    }

    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a> {
        Box::pin(anthropic::stream_response(
            resolved, req, cancel_rx, on_delta,
        ))
    }

    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a> {
        Box::pin(anthropic::fetch_remote_models(resolved))
    }

    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability {
        anthropic::get_reasoning_capability(model_id, cached_levels)
    }
}

static OPENAI_RESPONSES_ADAPTER: OpenAIResponsesAdapter = OpenAIResponsesAdapter;
static CHAT_COMPLETIONS_ADAPTER: ChatCompletionsAdapter = ChatCompletionsAdapter;
static GEMINI_ADAPTER: GeminiAdapter = GeminiAdapter;
static ANTHROPIC_ADAPTER: AnthropicAdapter = AnthropicAdapter;

pub(crate) fn adapter_for_kind(
    provider_kind: &str,
) -> Result<&'static dyn AgentJaxProviderApi, String> {
    let Some(transport_family) = registry::provider_transport_family(provider_kind) else {
        return Err(format!(
            "Unsupported provider kind '{}'. Register a provider plugin to enable it.",
            provider_kind
        ));
    };
    let adapters: [&dyn AgentJaxProviderApi; 4] = [
        &OPENAI_RESPONSES_ADAPTER,
        &CHAT_COMPLETIONS_ADAPTER,
        &GEMINI_ADAPTER,
        &ANTHROPIC_ADAPTER,
    ];

    for adapter in adapters {
        if adapter.transport_family() == transport_family {
            return Ok(adapter);
        }
    }

    Err(format!(
        "Provider kind '{}' uses transport family {:?}, but this AgentJax build has no matching adapter.",
        provider_kind, transport_family
    ))
}
