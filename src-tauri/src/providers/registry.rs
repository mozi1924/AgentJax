use crate::config::{ModelRequestConfig, ProviderConfig, ProviderModelConfig};
use crate::tools::ToolSchemaFormat;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

use super::capabilities::ProviderCapabilities;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransportFamily {
    Responses,
    ChatCompletions,
    CustomOauth,
}

#[derive(Debug, Clone)]
pub struct ProviderDefinition {
    pub kind: &'static str,
    pub display_name: &'static str,
    #[allow(dead_code)]
    pub transport_family: ProviderTransportFamily,
    pub default_api_endpoint: &'static str,
    pub default_realtime_endpoint: Option<&'static str>,
    pub default_credential_env: &'static str,
    pub default_supports_websockets: bool,
    pub default_stream_transport: &'static str,
    pub default_model_ids: &'static [&'static str],
    pub capabilities: ProviderCapabilities,
    pub tool_schema_format: ToolSchemaFormat,
}

impl ProviderDefinition {
    pub fn build_default_config(&self) -> ProviderConfig {
        let mut models = BTreeMap::new();
        for model_id in self.default_model_ids {
            models.insert(
                (*model_id).to_string(),
                ProviderModelConfig {
                    model: (*model_id).to_string(),
                    enabled: true,
                    request: ModelRequestConfig::default(),
                },
            );
        }

        ProviderConfig {
            kind: self.kind.to_string(),
            api_endpoint: self.default_api_endpoint.to_string(),
            models_endpoint_candidates: Vec::new(),
            query_params: BTreeMap::new(),
            http_headers: BTreeMap::new(),
            env_http_headers: BTreeMap::new(),
            realtime_endpoint: self
                .default_realtime_endpoint
                .map(|value| value.to_string()),
            supports_websockets: self.default_supports_websockets,
            stream_transport: self.default_stream_transport.to_string(),
            credential: None,
            credential_env: self.default_credential_env.to_string(),
            request_timeout_seconds: None,
            request_max_retries: None,
            stream_max_retries: None,
            stream_idle_timeout_ms: None,
            websocket_connect_timeout_ms: None,
            models,
        }
    }
}

pub fn builtin_provider_definitions() -> Vec<ProviderDefinition> {
    vec![ProviderDefinition {
        kind: "openai-responses",
        display_name: "OpenAI Responses",
        transport_family: ProviderTransportFamily::Responses,
        default_api_endpoint: "https://api.openai.com/v1",
        default_realtime_endpoint: None,
        default_credential_env: "OPENAI_API_KEY",
        default_supports_websockets: true,
        default_stream_transport: "websocket",
        default_model_ids: &["gpt-5-mini", "gpt-5"],
        capabilities: ProviderCapabilities::openai_responses(),
        tool_schema_format: ToolSchemaFormat::Responses,
    }]
}

pub fn provider_definition(provider_kind: &str) -> Option<ProviderDefinition> {
    let normalized = provider_kind.trim().to_lowercase();
    builtin_provider_definitions()
        .into_iter()
        .find(|definition| definition.kind == normalized)
}

pub fn default_provider_definition() -> ProviderDefinition {
    provider_definition("openai-responses")
        .unwrap_or_else(|| builtin_provider_definitions().into_iter().next().unwrap())
}

pub fn default_provider_kind() -> &'static str {
    default_provider_definition().kind
}

pub fn provider_kind_options() -> Vec<(String, String)> {
    builtin_provider_definitions()
        .into_iter()
        .map(|definition| {
            (
                definition.display_name.to_string(),
                definition.kind.to_string(),
            )
        })
        .collect()
}

pub fn default_provider_config() -> ProviderConfig {
    default_provider_definition().build_default_config()
}

pub fn provider_capabilities(provider_kind: &str) -> Option<ProviderCapabilities> {
    provider_definition(provider_kind).map(|definition| definition.capabilities)
}

pub fn provider_tool_schema_format(provider_kind: &str) -> Option<ToolSchemaFormat> {
    provider_definition(provider_kind).map(|definition| definition.tool_schema_format)
}
