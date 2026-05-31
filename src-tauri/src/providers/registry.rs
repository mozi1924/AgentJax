use crate::config::{ModelRequestConfig, ProviderConfig, ProviderModelConfig};
use crate::plugin_runtime::{PluginPackage, PluginProviderDefinition, builtin_plugin_packages};
use crate::tools::ToolSchemaFormat;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use super::capabilities::ProviderCapabilities;

/// Transport families implemented by AgentJax's provider API adapters.
///
/// Provider plugins declare one of these families in their manifest. The host
/// keeps concrete HTTP/SSE/WebSocket code in Rust adapters while allowing the
/// provider catalog, defaults, and settings schema to be contributed by plugins.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ProviderTransportFamily {
    Responses,
    ChatCompletions,
    Gemini,
    Anthropic,
    CustomOauth,
}

static DYNAMIC_REGISTRY: OnceLock<RwLock<Vec<DynamicProviderDefinition>>> = OnceLock::new();

fn get_registry() -> &'static RwLock<Vec<DynamicProviderDefinition>> {
    DYNAMIC_REGISTRY.get_or_init(|| RwLock::new(builtin_provider_definitions()))
}

#[derive(Debug, Clone)]
pub struct DynamicProviderDefinition {
    pub kind: String,
    pub display_name: String,
    pub transport_family: ProviderTransportFamily,
    pub config_schema: Value,
    pub capabilities: ProviderCapabilities,
    pub tool_schema_format: ToolSchemaFormat,
    pub default_model_ids: Vec<String>,
    pub default_config: ProviderConfig,
}

pub fn builtin_provider_definitions() -> Vec<DynamicProviderDefinition> {
    builtin_plugin_packages()
        .into_iter()
        .flat_map(|package| package.manifest.providers)
        .filter_map(|provider| {
            dynamic_provider_definition_from_plugin(provider)
                .map_err(|err| {
                    log::error!("Ignoring invalid built-in provider plugin declaration: {err}");
                    err
                })
                .ok()
        })
        .collect()
}

pub fn register_plugin_providers_from_packages(packages: impl IntoIterator<Item = PluginPackage>) {
    for package in packages {
        for provider in package.manifest.providers {
            register_plugin_provider(provider);
        }
    }
}

pub fn register_plugin_provider(plugin_provider: PluginProviderDefinition) {
    let Ok(definition) = dynamic_provider_definition_from_plugin(plugin_provider) else {
        return;
    };

    let mut registry = get_registry().write().unwrap();
    if registry
        .iter()
        .any(|existing| existing.kind == definition.kind)
    {
        return;
    }
    registry.push(definition);
}

#[allow(dead_code)]
pub fn unregister_plugin_provider(provider_kind: &str) {
    let mut registry = get_registry().write().unwrap();
    let normalized = normalize_provider_kind(provider_kind);
    registry.retain(|definition| definition.kind != normalized);
}

#[allow(dead_code)]
pub fn provider_definitions() -> Vec<DynamicProviderDefinition> {
    get_registry().read().unwrap().clone()
}

pub fn provider_definition(provider_kind: &str) -> Option<DynamicProviderDefinition> {
    let normalized = normalize_provider_kind(provider_kind);
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
        .map(|definition| (definition.display_name.clone(), definition.kind.clone()))
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

pub fn provider_transport_family(provider_kind: &str) -> Option<ProviderTransportFamily> {
    provider_definition(provider_kind).map(|definition| definition.transport_family)
}

fn dynamic_provider_definition_from_plugin(
    plugin_provider: PluginProviderDefinition,
) -> Result<DynamicProviderDefinition, String> {
    let kind = normalize_provider_kind(&plugin_provider.kind);
    if kind.is_empty() {
        return Err("provider kind cannot be empty".to_string());
    }
    let display_name = plugin_provider.display_name.trim().to_string();
    if display_name.is_empty() {
        return Err(format!("provider '{kind}' must have a display name"));
    }

    let transport_family = parse_transport_family(plugin_provider.transport_family.as_deref())?;
    let capabilities = plugin_provider
        .capabilities
        .as_ref()
        .and_then(parse_capabilities)
        .unwrap_or_else(|| default_capabilities_for_family(transport_family));
    let tool_schema_format = plugin_provider
        .tool_schema_format
        .as_deref()
        .and_then(parse_tool_schema_format)
        .unwrap_or_else(|| default_tool_schema_format_for_family(transport_family));
    let config_schema = normalize_config_schema(&plugin_provider);
    let default_model_ids = plugin_provider.default_model_ids;
    let default_config = build_default_config(&kind, &default_model_ids, &config_schema);

    Ok(DynamicProviderDefinition {
        kind,
        display_name,
        transport_family,
        config_schema,
        capabilities,
        tool_schema_format,
        default_model_ids,
        default_config,
    })
}

fn normalize_provider_kind(provider_kind: &str) -> String {
    provider_kind.trim().to_lowercase()
}

fn parse_transport_family(value: Option<&str>) -> Result<ProviderTransportFamily, String> {
    let normalized = value
        .unwrap_or("chat_completions")
        .trim()
        .to_ascii_lowercase()
        .replace('-', "_");
    match normalized.as_str() {
        "responses" | "openai_responses" => Ok(ProviderTransportFamily::Responses),
        "chat_completions" | "openai_chat_completions" => {
            Ok(ProviderTransportFamily::ChatCompletions)
        }
        "gemini" | "google_gemini" => Ok(ProviderTransportFamily::Gemini),
        "anthropic" | "claude" => Ok(ProviderTransportFamily::Anthropic),
        "custom_oauth" => Ok(ProviderTransportFamily::CustomOauth),
        _ => Err(format!(
            "unsupported provider transport family '{normalized}'"
        )),
    }
}

fn parse_capabilities(value: &Value) -> Option<ProviderCapabilities> {
    serde_json::from_value::<ProviderCapabilities>(value.clone()).ok()
}

fn parse_tool_schema_format(value: &str) -> Option<ToolSchemaFormat> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "responses" | "openai_responses" => Some(ToolSchemaFormat::Responses),
        "chat_completions" | "openai_chat_completions" => Some(ToolSchemaFormat::ChatCompletions),
        "gemini" => Some(ToolSchemaFormat::Gemini),
        "anthropic" | "claude" => Some(ToolSchemaFormat::Anthropic),
        _ => None,
    }
}

fn default_capabilities_for_family(family: ProviderTransportFamily) -> ProviderCapabilities {
    match family {
        ProviderTransportFamily::Responses => ProviderCapabilities::openai_responses(),
        ProviderTransportFamily::ChatCompletions | ProviderTransportFamily::CustomOauth => {
            ProviderCapabilities::chat_completions()
        }
        ProviderTransportFamily::Gemini => ProviderCapabilities::gemini(),
        ProviderTransportFamily::Anthropic => ProviderCapabilities::anthropic(),
    }
}

fn default_tool_schema_format_for_family(family: ProviderTransportFamily) -> ToolSchemaFormat {
    match family {
        ProviderTransportFamily::Responses => ToolSchemaFormat::Responses,
        ProviderTransportFamily::ChatCompletions | ProviderTransportFamily::CustomOauth => {
            ToolSchemaFormat::ChatCompletions
        }
        ProviderTransportFamily::Gemini => ToolSchemaFormat::Gemini,
        ProviderTransportFamily::Anthropic => ToolSchemaFormat::Anthropic,
    }
}

fn normalize_config_schema(plugin_provider: &PluginProviderDefinition) -> Value {
    let has_properties = plugin_provider
        .config_schema
        .get("properties")
        .and_then(Value::as_object)
        .is_some_and(|properties| !properties.is_empty());
    if has_properties {
        return plugin_provider.config_schema.clone();
    }

    standard_provider_config_schema(plugin_provider)
}

fn standard_provider_config_schema(plugin_provider: &PluginProviderDefinition) -> Value {
    json!({
        "type": "object",
        "properties": {
            "apiEndpoint": {
                "type": "string",
                "default": plugin_provider.default_api_endpoint,
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
                "default": plugin_provider.default_realtime_endpoint
            },
            "supportsWebsockets": {
                "type": "boolean",
                "default": plugin_provider.default_supports_websockets
            },
            "streamTransport": {
                "type": "string",
                "default": plugin_provider.default_stream_transport
            },
            "credential": {
                "type": ["string", "null"],
                "default": null,
                "sensitive": true
            },
            "credentialEnv": {
                "type": "string",
                "default": plugin_provider.default_credential_env
            }
        }
    })
}

fn build_default_config(
    kind: &str,
    default_model_ids: &[String],
    config_schema: &Value,
) -> ProviderConfig {
    let mut models = BTreeMap::new();
    for model_id in default_model_ids {
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
    if let Some(properties) = config_schema.get("properties").and_then(Value::as_object) {
        for (key, schema_val) in properties {
            let default_val = schema_val.get("default").cloned().unwrap_or(Value::Null);
            custom_settings.insert(key.clone(), default_val);
        }
    }

    ProviderConfig {
        kind: kind.to_string(),
        models,
        custom_settings,
    }
}
