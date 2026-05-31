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
    Gemini,
    Anthropic,
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

        let mut custom_settings = serde_json::Map::new();
        custom_settings.insert("apiEndpoint".to_string(), serde_json::Value::String(self.default_api_endpoint.to_string()));
        custom_settings.insert("modelsEndpointCandidates".to_string(), serde_json::Value::Array(Vec::new()));
        custom_settings.insert("queryParams".to_string(), serde_json::to_value(BTreeMap::<String, String>::new()).unwrap());
        custom_settings.insert("httpHeaders".to_string(), serde_json::to_value(BTreeMap::<String, String>::new()).unwrap());
        custom_settings.insert("envHttpHeaders".to_string(), serde_json::to_value(BTreeMap::<String, String>::new()).unwrap());
        custom_settings.insert("realtimeEndpoint".to_string(), match self.default_realtime_endpoint {
            Some(val) => serde_json::Value::String(val.to_string()),
            None => serde_json::Value::Null,
        });
        custom_settings.insert("supportsWebsockets".to_string(), serde_json::Value::Bool(self.default_supports_websockets));
        custom_settings.insert("streamTransport".to_string(), serde_json::Value::String(self.default_stream_transport.to_string()));
        custom_settings.insert("credential".to_string(), serde_json::Value::Null);
        custom_settings.insert("credentialEnv".to_string(), serde_json::Value::String(self.default_credential_env.to_string()));

        ProviderConfig {
            kind: self.kind.to_string(),
            models,
            custom_settings,
        }
    }
}

pub fn builtin_provider_definitions() -> Vec<ProviderDefinition> {
    vec![
        ProviderDefinition {
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
        },
        ProviderDefinition {
            kind: "chat-completions",
            display_name: "Chat Completions",
            transport_family: ProviderTransportFamily::ChatCompletions,
            default_api_endpoint: "https://api.openai.com/v1",
            default_realtime_endpoint: None,
            default_credential_env: "OPENAI_API_KEY",
            default_supports_websockets: false,
            default_stream_transport: "sse",
            default_model_ids: &["gpt-4.1", "gpt-4o"],
            capabilities: ProviderCapabilities::chat_completions(),
            tool_schema_format: ToolSchemaFormat::ChatCompletions,
        },
        ProviderDefinition {
            kind: "gemini",
            display_name: "Gemini",
            transport_family: ProviderTransportFamily::Gemini,
            default_api_endpoint: "https://generativelanguage.googleapis.com/v1beta",
            default_realtime_endpoint: None,
            default_credential_env: "GEMINI_API_KEY",
            default_supports_websockets: false,
            default_stream_transport: "sse",
            default_model_ids: &["gemini-2.5-flash", "gemini-2.5-pro"],
            capabilities: ProviderCapabilities::gemini(),
            tool_schema_format: ToolSchemaFormat::Gemini,
        },
        ProviderDefinition {
            kind: "anthropic",
            display_name: "Anthropic",
            transport_family: ProviderTransportFamily::Anthropic,
            default_api_endpoint: "https://api.anthropic.com/v1",
            default_realtime_endpoint: None,
            default_credential_env: "ANTHROPIC_API_KEY",
            default_supports_websockets: false,
            default_stream_transport: "sse",
            default_model_ids: &["claude-sonnet-4-5", "claude-opus-4-1"],
            capabilities: ProviderCapabilities::anthropic(),
            tool_schema_format: ToolSchemaFormat::Anthropic,
        },
    ]
}

use std::sync::{OnceLock, RwLock};

static DYNAMIC_REGISTRY: OnceLock<RwLock<Vec<DynamicProviderDefinition>>> = OnceLock::new();

fn get_registry() -> &'static RwLock<Vec<DynamicProviderDefinition>> {
    DYNAMIC_REGISTRY.get_or_init(|| {
        RwLock::new(
            builtin_provider_definitions()
                .into_iter()
                .map(|builtin| DynamicProviderDefinition {
                    kind: builtin.kind.to_string(),
                    display_name: builtin.display_name.to_string(),
                    config_schema: builtin.config_schema(),
                    capabilities: builtin.capabilities,
                    tool_schema_format: builtin.tool_schema_format,
                    default_model_ids: builtin.default_model_ids.iter().map(|s| s.to_string()).collect(),
                    default_config: builtin.build_default_config(),
                })
                .collect()
        )
    })
}

#[derive(Debug, Clone)]
pub struct DynamicProviderDefinition {
    pub kind: String,
    pub display_name: String,
    pub config_schema: serde_json::Value,
    pub capabilities: ProviderCapabilities,
    pub tool_schema_format: ToolSchemaFormat,
    pub default_model_ids: Vec<String>,
    pub default_config: ProviderConfig,
}

impl ProviderDefinition {
    pub fn config_schema(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "apiEndpoint": {
                    "type": "string",
                    "default": self.default_api_endpoint,
                    "title": "API Endpoint"
                },
                "modelsEndpointCandidates": {
                    "type": "array",
                    "items": { "type": "string" },
                    "default": []
                },
                "queryParams": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "default": {}
                },
                "httpHeaders": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "default": {}
                },
                "envHttpHeaders": {
                    "type": "object",
                    "additionalProperties": { "type": "string" },
                    "default": {}
                },
                "realtimeEndpoint": {
                    "type": ["string", "null"],
                    "default": self.default_realtime_endpoint
                },
                "supportsWebsockets": {
                    "type": "boolean",
                    "default": self.default_supports_websockets
                },
                "streamTransport": {
                    "type": "string",
                    "default": self.default_stream_transport
                },
                "credential": {
                    "type": ["string", "null"],
                    "default": null,
                    "sensitive": true
                },
                "credentialEnv": {
                    "type": "string",
                    "default": self.default_credential_env
                }
            }
        })
    }
}

pub fn register_plugin_provider(
    plugin_provider: crate::plugin_runtime::PluginProviderDefinition,
) {
    let mut registry = get_registry().write().unwrap();
    let kind = plugin_provider.kind.trim().to_lowercase();
    if registry.iter().any(|d| d.kind == kind) {
        return;
    }

    let mut models = BTreeMap::new();
    for model_id in &plugin_provider.default_model_ids {
        models.insert(
            model_id.clone(),
            ProviderModelConfig {
                model: model_id.clone(),
                enabled: true,
                request: ModelRequestConfig::default(),
            },
        );
    }

    let mut custom_settings = serde_json::Map::new();
    if let Some(obj) = plugin_provider.config_schema.as_object() {
        if let Some(properties) = obj.get("properties").and_then(|p| p.as_object()) {
            for (key, schema_val) in properties {
                let default_val = schema_val.get("default").cloned().unwrap_or(serde_json::Value::Null);
                custom_settings.insert(key.clone(), default_val);
            }
        }
    }

    let default_config = ProviderConfig {
        kind: kind.clone(),
        models,
        custom_settings,
    };

    registry.push(DynamicProviderDefinition {
        kind: kind.clone(),
        display_name: plugin_provider.display_name,
        config_schema: plugin_provider.config_schema,
        capabilities: ProviderCapabilities::chat_completions(), // Fallback capabilities
        tool_schema_format: ToolSchemaFormat::ChatCompletions, // Fallback tools format
        default_model_ids: plugin_provider.default_model_ids,
        default_config,
    });
}

pub fn unregister_plugin_provider(provider_kind: &str) {
    let mut registry = get_registry().write().unwrap();
    let normalized = provider_kind.trim().to_lowercase();
    registry.retain(|d| d.kind != normalized);
}

pub fn provider_definitions() -> Vec<DynamicProviderDefinition> {
    get_registry().read().unwrap().clone()
}

pub fn provider_definition(provider_kind: &str) -> Option<DynamicProviderDefinition> {
    let normalized = provider_kind.trim().to_lowercase();
    get_registry()
        .read()
        .unwrap()
        .iter()
        .find(|definition| definition.kind == normalized)
        .cloned()
}

pub fn default_provider_definition() -> DynamicProviderDefinition {
    provider_definition("openai-responses")
        .unwrap_or_else(|| get_registry().read().unwrap().first().unwrap().clone())
}

pub fn default_provider_kind() -> String {
    default_provider_definition().kind
}

pub fn provider_kind_options() -> Vec<(String, String)> {
    get_registry()
        .read()
        .unwrap()
        .iter()
        .map(|definition| {
            (
                definition.display_name.clone(),
                definition.kind.clone(),
            )
        })
        .collect()
}

pub fn default_provider_config() -> ProviderConfig {
    default_provider_definition().default_config
}

pub fn provider_capabilities(provider_kind: &str) -> Option<ProviderCapabilities> {
    provider_definition(provider_kind).map(|definition| definition.capabilities)
}

pub fn provider_tool_schema_format(provider_kind: &str) -> Option<ToolSchemaFormat> {
    provider_definition(provider_kind).map(|definition| definition.tool_schema_format)
}
