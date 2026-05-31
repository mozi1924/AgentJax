pub mod capabilities;
pub mod core;
pub mod network;
mod plugin_host;
pub mod registry;
pub mod types;

use serde_json::Value;

pub use capabilities::ProviderCapabilities;
pub use types::{
    ModelReasoningCapability, ProviderModelDescriptor, ProviderPendingToolCall,
    ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};

pub fn get_capabilities(provider_kind: &str) -> Result<ProviderCapabilities, String> {
    registry::provider_capabilities(provider_kind)
        .ok_or_else(|| format!("Unsupported provider kind '{}'. Register a provider plugin to enable it.", provider_kind))
}

pub fn get_tool_schema_format(provider_kind: &str) -> Result<crate::tools::ToolSchemaFormat, String> {
    registry::provider_tool_schema_format(provider_kind)
        .ok_or_else(|| format!("Unsupported provider kind '{}'. Register a provider plugin to enable it.", provider_kind))
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

pub async fn stream_response<F>(
    config: &crate::config::AppConfig,
    req: &ResponseStreamRequest,
    cancel_rx: &mut tokio::sync::watch::Receiver<bool>,
    on_delta: F,
) -> Result<ResponseStreamResult, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String> + Send,
{
    plugin_host::stream_response(config, req, cancel_rx, on_delta).await
}

pub async fn fetch_remote_models(
    config: &crate::config::AppConfig,
    provider_key: &str,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    plugin_host::fetch_remote_models(config, provider_key).await
}

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> Result<ModelReasoningCapability, String> {
    plugin_host::get_reasoning_capability(provider_kind, model_id, cached_levels)
}
