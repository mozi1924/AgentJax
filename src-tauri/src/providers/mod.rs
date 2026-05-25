pub mod capabilities;
mod codex;
mod openai_standard;
mod responses;
pub mod types;

use std::future::Future;
use std::pin::Pin;

use tokio::sync::watch;

use crate::config::{AppConfig, ResolvedModelConfig};
use crate::tools::ToolSchemaFormat;
use capabilities::ProviderCapabilities;
use serde_json::Value;
use types::{
    ModelReasoningCapability, ProviderEventSink, ProviderModelDescriptor, ProviderPendingToolCall,
    ProviderStreamEvent, ResponseStreamRequest, ResponseStreamResult,
};

type StreamFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ResponseStreamResult, String>> + Send + 'a>>;
type ModelsFuture<'a> =
    Pin<Box<dyn Future<Output = Result<Vec<ProviderModelDescriptor>, String>> + Send + 'a>>;

trait ProviderAdapter: Send + Sync {
    fn matches_kind(&self, provider_kind: &str) -> bool;
    fn capabilities(&self) -> ProviderCapabilities;
    fn tool_schema_format(&self) -> ToolSchemaFormat;
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
    fn extract_pending_tool_calls(&self, output_items: &[Value]) -> Vec<ProviderPendingToolCall>;
    fn build_tool_result_input_item(&self, call_id: &str, output: &str) -> Value;
    fn build_user_input_item(&self, text: &str) -> Value;
    fn compose_tool_continuation_input(
        &self,
        output_items: &[Value],
        tool_results_items: Vec<Value>,
    ) -> Vec<Value>;
}

struct CodexAdapter;

impl ProviderAdapter for CodexAdapter {
    fn matches_kind(&self, provider_kind: &str) -> bool {
        matches!(provider_kind, "codex" | "openai-codex" | "openai_codex")
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::codex()
    }

    fn tool_schema_format(&self) -> ToolSchemaFormat {
        ToolSchemaFormat::Responses
    }

    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a> {
        Box::pin(codex::stream_response(resolved, req, cancel_rx, on_delta))
    }

    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a> {
        Box::pin(codex::fetch_remote_models(resolved))
    }

    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability {
        codex::get_reasoning_capability(model_id, cached_levels)
    }

    fn extract_pending_tool_calls(&self, output_items: &[Value]) -> Vec<ProviderPendingToolCall> {
        responses::extract_pending_tool_calls_from_output(output_items)
    }

    fn build_tool_result_input_item(&self, call_id: &str, output: &str) -> Value {
        responses::build_tool_result_input_item(call_id, output)
    }

    fn build_user_input_item(&self, text: &str) -> Value {
        responses::build_user_input_item(text)
    }

    fn compose_tool_continuation_input(
        &self,
        output_items: &[Value],
        tool_results_items: Vec<Value>,
    ) -> Vec<Value> {
        responses::compose_tool_continuation_input(output_items, tool_results_items)
    }
}

struct OpenAIStandardAdapter;

impl ProviderAdapter for OpenAIStandardAdapter {
    fn matches_kind(&self, provider_kind: &str) -> bool {
        matches!(
            provider_kind,
            "openai" | "openai-standard" | "openai_standard"
        )
    }

    fn capabilities(&self) -> ProviderCapabilities {
        ProviderCapabilities::openai_standard()
    }

    fn tool_schema_format(&self) -> ToolSchemaFormat {
        ToolSchemaFormat::Responses
    }

    fn stream_response<'a>(
        &'a self,
        resolved: &'a ResolvedModelConfig,
        req: &'a ResponseStreamRequest,
        cancel_rx: &'a mut watch::Receiver<bool>,
        on_delta: &'a mut ProviderEventSink<'a>,
    ) -> StreamFuture<'a> {
        Box::pin(openai_standard::stream_response(
            resolved, req, cancel_rx, on_delta,
        ))
    }

    fn fetch_remote_models<'a>(&'a self, resolved: &'a ResolvedModelConfig) -> ModelsFuture<'a> {
        Box::pin(openai_standard::fetch_remote_models(resolved))
    }

    fn reasoning_capability(
        &self,
        model_id: &str,
        cached_levels: Option<&[String]>,
    ) -> ModelReasoningCapability {
        openai_standard::get_reasoning_capability(model_id, cached_levels)
    }

    fn extract_pending_tool_calls(&self, output_items: &[Value]) -> Vec<ProviderPendingToolCall> {
        responses::extract_pending_tool_calls_from_output(output_items)
    }

    fn build_tool_result_input_item(&self, call_id: &str, output: &str) -> Value {
        responses::build_tool_result_input_item(call_id, output)
    }

    fn build_user_input_item(&self, text: &str) -> Value {
        responses::build_user_input_item(text)
    }

    fn compose_tool_continuation_input(
        &self,
        output_items: &[Value],
        tool_results_items: Vec<Value>,
    ) -> Vec<Value> {
        responses::compose_tool_continuation_input(output_items, tool_results_items)
    }
}

static CODEX_ADAPTER: CodexAdapter = CodexAdapter;
static OPENAI_STANDARD_ADAPTER: OpenAIStandardAdapter = OpenAIStandardAdapter;

fn adapter_for_kind(provider_kind: &str) -> Result<&'static dyn ProviderAdapter, String> {
    let normalized = provider_kind.trim().to_lowercase();
    let adapters: [&dyn ProviderAdapter; 2] = [&CODEX_ADAPTER, &OPENAI_STANDARD_ADAPTER];

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

pub fn get_capabilities(provider_kind: &str) -> Result<ProviderCapabilities, String> {
    Ok(adapter_for_kind(provider_kind)?.capabilities())
}

pub fn get_tool_schema_format(provider_kind: &str) -> Result<ToolSchemaFormat, String> {
    Ok(adapter_for_kind(provider_kind)?.tool_schema_format())
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
    let adapter = adapter_for_kind(&resolved.provider.kind)?;
    adapter
        .stream_response(&resolved, req, cancel_rx, &mut on_delta)
        .await
}

pub async fn fetch_remote_models(
    config: &AppConfig,
    provider_key: &str,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let provider = config.resolved_provider(provider_key)?;
    let resolved = ResolvedModelConfig {
        profile_key: "<catalog-sync>".to_string(),
        provider_key: provider_key.to_string(),
        model_ref: format!("{}/<catalog-sync>", provider_key),
        system_prompt: config.system_prompt.clone(),
        timeout_seconds: provider.resolved_timeout_seconds(config.request_timeout_seconds),
        provider,
        model_id: "".to_string(),
        request: Default::default(),
    };

    let adapter = adapter_for_kind(&resolved.provider.kind)?;
    adapter.fetch_remote_models(&resolved).await
}

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> Result<ModelReasoningCapability, String> {
    Ok(adapter_for_kind(provider_kind)?.reasoning_capability(model_id, cached_levels))
}

pub fn extract_pending_tool_calls(
    provider_kind: &str,
    output_items: &[Value],
) -> Result<Vec<ProviderPendingToolCall>, String> {
    Ok(adapter_for_kind(provider_kind)?.extract_pending_tool_calls(output_items))
}

pub fn build_tool_result_input_item(
    provider_kind: &str,
    call_id: &str,
    output: &str,
) -> Result<Value, String> {
    Ok(adapter_for_kind(provider_kind)?.build_tool_result_input_item(call_id, output))
}

pub fn build_user_input_item(provider_kind: &str, text: &str) -> Result<Value, String> {
    Ok(adapter_for_kind(provider_kind)?.build_user_input_item(text))
}

pub fn compose_tool_continuation_input(
    provider_kind: &str,
    output_items: &[Value],
    tool_results_items: Vec<Value>,
) -> Result<Vec<Value>, String> {
    Ok(adapter_for_kind(provider_kind)?
        .compose_tool_continuation_input(output_items, tool_results_items))
}
