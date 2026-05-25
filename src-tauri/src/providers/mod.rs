pub mod capabilities;
mod codex;
mod openai_standard;
pub mod types;

use tokio::sync::watch;

use crate::config::{AppConfig, ResolvedModelConfig};
use types::{
    ModelReasoningCapability, ProviderModelDescriptor, ProviderStreamEvent, ResponseStreamRequest,
    ResponseStreamResult,
};

#[derive(Debug, Clone, Copy)]
enum ProviderKind {
    Codex,
    OpenAIStandard,
}

impl ProviderKind {
    fn from_provider_kind(kind: &str) -> Result<Self, String> {
        match kind.trim().to_lowercase().as_str() {
            "openai" | "codex" => Ok(Self::Codex),
            "openai-standard" | "openai_standard" => Ok(Self::OpenAIStandard),
            other => Err(format!(
                "Unsupported provider kind '{}'. Add an adapter under src-tauri/src/providers to enable it.",
                other
            )),
        }
    }
}

pub async fn stream_response<F>(
    config: &AppConfig,
    req: &ResponseStreamRequest,
    tools_catalog: Option<&crate::tools::ToolCatalog>,
    cancel_rx: &mut watch::Receiver<bool>,
    on_delta: F,
) -> Result<ResponseStreamResult, String>
where
    F: FnMut(ProviderStreamEvent) -> Result<(), String>,
{
    let resolved = config.resolve_model_profile(req.model.as_deref())?;

    match ProviderKind::from_provider_kind(&resolved.provider.kind)? {
        ProviderKind::Codex => {
            codex::stream_response(&resolved, req, tools_catalog, cancel_rx, on_delta).await
        }
        ProviderKind::OpenAIStandard => {
            openai_standard::stream_response(&resolved, req, tools_catalog, cancel_rx, on_delta)
                .await
        }
    }
}

pub async fn fetch_remote_models(
    config: &AppConfig,
    provider_key: &str,
) -> Result<Vec<ProviderModelDescriptor>, String> {
    let provider = config.resolved_provider(provider_key)?;
    let resolved = ResolvedModelConfig {
        profile_key: "<catalog-sync>".to_string(),
        provider_key: provider_key.to_string(),
        timeout_seconds: provider.resolved_timeout_seconds(config.request_timeout_seconds),
        provider,
        model_id: "".to_string(),
        request: Default::default(),
    };

    match ProviderKind::from_provider_kind(&resolved.provider.kind)? {
        ProviderKind::Codex => codex::fetch_remote_models(&resolved).await,
        ProviderKind::OpenAIStandard => openai_standard::fetch_remote_models(&resolved).await,
    }
}

pub fn get_reasoning_capability(
    provider_kind: &str,
    model_id: &str,
    cached_levels: Option<&[String]>,
) -> Result<ModelReasoningCapability, String> {
    match ProviderKind::from_provider_kind(provider_kind)? {
        ProviderKind::Codex => Ok(codex::get_reasoning_capability(model_id, cached_levels)),
        ProviderKind::OpenAIStandard => Ok(openai_standard::get_reasoning_capability(
            model_id,
            cached_levels,
        )),
    }
}
