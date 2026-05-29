mod adapters;
pub mod capabilities;
mod chat_completions;
pub mod core;
mod openai_responses;
pub mod registry;
mod responses;
mod sse;
pub mod types;

use tokio::sync::watch;

use crate::config::{AppConfig, ResolvedModelConfig};
use crate::tools::ToolSchemaFormat;
use capabilities::ProviderCapabilities;
use serde_json::Value;
use types::{
    ModelReasoningCapability, ProviderModelDescriptor, ProviderPendingToolCall,
    ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};

pub fn get_capabilities(provider_kind: &str) -> Result<ProviderCapabilities, String> {
    registry::provider_capabilities(provider_kind)
        .ok_or_else(|| format!("Unsupported provider kind '{}'. Add an adapter under src-tauri/src/providers to enable it.", provider_kind))
}

pub fn get_tool_schema_format(provider_kind: &str) -> Result<ToolSchemaFormat, String> {
    registry::provider_tool_schema_format(provider_kind)
        .ok_or_else(|| format!("Unsupported provider kind '{}'. Add an adapter under src-tauri/src/providers to enable it.", provider_kind))
}

pub async fn stream_response<F>(
    config: &AppConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut watch::Receiver<bool>,
    mut on_delta: F,
) -> Result<ResponseStreamResult, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    let resolved = config.resolve_model_profile(req.model.as_deref())?;
    let adapter = adapters::adapter_for_kind(&resolved.provider.kind)?;
    adapter
        .stream_response(&resolved, req, cancel_rx, &mut on_delta)
        .await
}

pub async fn fetch_remote_models(
    config: &AppConfig,
    provider_key: &str,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let provider = config.resolved_provider(provider_key)?;
    let prompt_assembly = config.compile_prompt_assembly();
    let resolved = ResolvedModelConfig {
        profile_key: "<catalog-sync>".to_string(),
        provider_key: provider_key.to_string(),
        model_ref: format!("{}/<catalog-sync>", provider_key),
        system_prompt: prompt_assembly.instructions_text.clone(),
        prompt_assembly,
        timeout_seconds: provider.resolved_timeout_seconds(config.request_timeout_seconds),
        provider,
        model_id: "".to_string(),
        request: Default::default(),
    };

    let adapter = adapters::adapter_for_kind(&resolved.provider.kind)?;
    adapter.fetch_remote_models(&resolved).await
}

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> Result<ModelReasoningCapability, String> {
    Ok(adapters::adapter_for_kind(provider_kind)?.reasoning_capability(model_id, cached_levels))
}

pub fn extract_pending_tool_calls(
    _provider_kind: &str,
    output_items: &[Value],
) -> Result<Vec<ProviderPendingToolCall>, String> {
    Ok(core::extract_pending_tool_calls_from_output(output_items))
}

pub fn build_tool_result_input_item(
    _provider_kind: &str,
    call_id: &str,
    output: &str,
) -> Result<Value, String> {
    Ok(core::build_tool_result_input_item(call_id, output))
}

pub fn build_user_input_item(_provider_kind: &str, text: &str) -> Result<Value, String> {
    Ok(core::build_user_input_item(text))
}

pub fn compose_tool_continuation_input(
    _provider_kind: &str,
    output_items: &[Value],
    tool_results_items: Vec<Value>,
) -> Result<Vec<Value>, String> {
    Ok(core::compose_tool_continuation_input(
        output_items,
        tool_results_items,
    ))
}
