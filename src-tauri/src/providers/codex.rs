use tokio::sync::watch;

use super::capabilities::ProviderCapabilities;
use super::responses;
use super::types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ResponseStreamRequest,
    ResponseStreamResult,
};
use crate::config::ResolvedModelConfig;

fn stream_behavior() -> responses::stream::ResponsesStreamBehavior {
    responses::stream::ResponsesStreamBehavior {
        api_label: "Codex",
        capabilities: ProviderCapabilities::codex(),
        force_store_false: true,
        retry_store_false: false,
    }
}

pub async fn fetch_remote_models(
    resolved: &ResolvedModelConfig,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let strategy = responses::models::ModelsFetchStrategy::openai_compatible()
        .with_provider_overrides(&resolved.provider.models_endpoint_candidates);
    responses::models::fetch_remote_models_with_strategy(resolved, &strategy).await
}

pub async fn stream_response(
    resolved: &ResolvedModelConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: &mut ProviderEventSink<'_>,
) -> Result<ResponseStreamResult, String> {
    responses::stream::stream_response_with_behavior(
        resolved,
        req,
        stream_behavior(),
        cancel_rx,
        on_delta,
    )
    .await
}

pub fn get_reasoning_capability(
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> ModelReasoningCapability {
    let supported_reasoning_levels = cached_levels
        .map(responses::normalize_reasoning_levels)
        .filter(|levels| !levels.is_empty())
        .unwrap_or_else(|| fallback_reasoning_levels(model_id));

    ModelReasoningCapability {
        supports_reasoning: !supported_reasoning_levels.is_empty(),
        supported_reasoning_levels,
    }
}

fn fallback_reasoning_levels(model_id: &str) -> Vec<String> {
    responses::infer_reasoning_levels_from_model_id(model_id)
}
