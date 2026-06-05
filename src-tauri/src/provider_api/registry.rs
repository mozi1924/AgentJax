use crate::agentjax_err;
use crate::config::{ModelRequestConfig, ProviderConfig, ProviderModelConfig};
use crate::plugin_runtime::{
    BuiltinModelDescriptor, ModelRoutingRule, PluginPackage, PluginProviderDefinition,
    builtin_plugin_packages, provider_definitions_for_package,
};
use crate::tools::ToolSchemaFormat;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::{OnceLock, RwLock};

use super::capabilities::ProviderCapabilities;

static DYNAMIC_REGISTRY: OnceLock<RwLock<Vec<DynamicProviderDefinition>>> = OnceLock::new();

fn get_registry() -> &'static RwLock<Vec<DynamicProviderDefinition>> {
    DYNAMIC_REGISTRY.get_or_init(|| RwLock::new(builtin_provider_definitions()))
}

#[derive(Debug, Clone)]
pub struct DynamicProviderDefinition {
    pub kind: String,
    pub display_name: String,
    pub default_priority: i64,
    pub plugin_package: Option<PluginPackage>,
    pub config_schema: Value,
    pub capabilities: ProviderCapabilities,
    pub tool_schema_format: ToolSchemaFormat,
    #[allow(dead_code)]
    pub default_model_ids: Vec<String>,
    pub default_config: ProviderConfig,
    pub supports_protocols: Vec<String>,

    // ── Phase 2: declarative fields ─────────────────────────────────────
    /// Ordered glob-based model routing rules.
    #[allow(dead_code)]
    pub model_routing: Vec<ModelRoutingRule>,
    /// Built-in model descriptors.
    pub builtin_models: Vec<BuiltinModelDescriptor>,
}

pub fn builtin_provider_definitions() -> Vec<DynamicProviderDefinition> {
    let mut definitions = builtin_plugin_packages()
        .into_iter()
        .flat_map(|package| match provider_definitions_for_package(&package) {
            Ok(providers) => providers
                .into_iter()
                .map(|provider| (package.clone(), provider))
                .collect::<Vec<_>>(),
            Err(err) => {
                log::error!(
                    "Ignoring provider definitions from built-in plugin '{}': {}",
                    package.manifest.id,
                    err
                );
                Vec::new()
            }
        })
        .filter_map(|(package, provider)| {
            dynamic_provider_definition_from_plugin(provider, Some(package))
                .map_err(|err| {
                    log::error!("Ignoring invalid built-in provider plugin declaration: {err}");
                    err
                })
                .ok()
        })
        .collect::<Vec<_>>();
    sort_provider_definitions(&mut definitions);
    definitions
}

pub fn register_plugin_providers_from_packages(packages: impl IntoIterator<Item = PluginPackage>) {
    for package in packages {
        let providers = match provider_definitions_for_package(&package) {
            Ok(providers) => providers,
            Err(err) => {
                log::warn!(
                    "Failed to load provider definitions from plugin '{}': {}",
                    package.manifest.id,
                    err
                );
                continue;
            }
        };
        for provider in providers {
            register_plugin_provider_from_package(package.clone(), provider);
        }
    }
}

#[allow(dead_code)] // Reserved for future use
pub fn register_plugin_provider(plugin_provider: PluginProviderDefinition) {
    let Ok(definition) = dynamic_provider_definition_from_plugin(plugin_provider, None) else {
        return;
    };

    insert_provider_definition(definition);
}

pub fn register_plugin_provider_from_package(
    package: PluginPackage,
    plugin_provider: PluginProviderDefinition,
) {
    let Ok(definition) = dynamic_provider_definition_from_plugin(plugin_provider, Some(package))
    else {
        return;
    };

    insert_provider_definition(definition);
}

fn insert_provider_definition(definition: DynamicProviderDefinition) {
    let mut registry = get_registry().write().unwrap();
    if registry
        .iter()
        .any(|existing| existing.kind == definition.kind)
    {
        return;
    }
    registry.push(definition);
    sort_provider_definitions(&mut registry);
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

pub fn provider_kind_options() -> Vec<(String, String)> {
    get_registry()
        .read()
        .unwrap()
        .iter()
        .map(|definition| (definition.display_name.clone(), definition.kind.clone()))
        .collect()
}

pub fn provider_capabilities(provider_kind: &str) -> Option<ProviderCapabilities> {
    provider_definition(provider_kind).map(|definition| definition.capabilities)
}

pub fn provider_tool_schema_format(provider_kind: &str) -> Option<ToolSchemaFormat> {
    provider_definition(provider_kind).map(|definition| definition.tool_schema_format)
}

pub fn provider_plugin_package(provider_kind: &str) -> Option<PluginPackage> {
    provider_definition(provider_kind).and_then(|definition| definition.plugin_package)
}

pub fn provider_supports_protocols(provider_kind: &str) -> Vec<String> {
    provider_definition(provider_kind)
        .map(|def| def.supports_protocols)
        .unwrap_or_default()
}

fn dynamic_provider_definition_from_plugin(
    plugin_provider: PluginProviderDefinition,
    package: Option<PluginPackage>,
) -> crate::error::AgentJaxResult<DynamicProviderDefinition> {
    let kind = normalize_provider_kind(&plugin_provider.kind);
    if kind.is_empty() {
        return Err(agentjax_err!("provider kind cannot be empty", Config));
    }
    let display_name = plugin_provider.display_name.trim().to_string();
    if display_name.is_empty() {
        return Err(agentjax_err!(
            format!("provider '{kind}' must have a display name"),
            Config
        ));
    }

    let capabilities = plugin_provider
        .capabilities
        .as_ref()
        .and_then(parse_capabilities)
        .ok_or_else(|| format!("provider '{kind}' must declare capabilities"))?;
    let tool_schema_format = plugin_provider
        .tool_schema_format
        .as_deref()
        .and_then(parse_tool_schema_format)
        .ok_or_else(|| format!("provider '{kind}' must declare a supported toolSchemaFormat"))?;
    let config_schema = plugin_provider.config_schema;
    let default_model_ids = plugin_provider.default_model_ids;
    let default_priority = plugin_provider.default_priority.unwrap_or(1000);
    let default_config = build_default_config(&kind, &default_model_ids, &config_schema);

    // Derive supports_protocols from model_routing if available and not
    // explicitly set, so downstream consumers (resolve_protocol fallback)
    // continue to work without changes.
    let supports_protocols = if !plugin_provider.supports_protocols.is_empty() {
        plugin_provider.supports_protocols.clone()
    } else {
        let mut protocols: Vec<String> = plugin_provider
            .model_routing
            .iter()
            .map(|r| r.protocol.clone())
            .collect();
        protocols.sort();
        protocols.dedup();
        protocols
    };

    Ok(DynamicProviderDefinition {
        kind,
        display_name,
        default_priority,
        plugin_package: package,
        config_schema,
        capabilities,
        tool_schema_format,
        default_model_ids,
        default_config,
        supports_protocols,
        model_routing: plugin_provider.model_routing,
        builtin_models: plugin_provider.builtin_models,
    })
}

fn sort_provider_definitions(definitions: &mut [DynamicProviderDefinition]) {
    definitions.sort_by(|a, b| {
        a.default_priority
            .cmp(&b.default_priority)
            .then_with(|| a.display_name.cmp(&b.display_name))
            .then_with(|| a.kind.cmp(&b.kind))
    });
}

fn normalize_provider_kind(provider_kind: &str) -> String {
    provider_kind.trim().to_lowercase()
}

fn parse_capabilities(value: &Value) -> Option<ProviderCapabilities> {
    serde_json::from_value::<ProviderCapabilities>(value.clone()).ok()
}

fn parse_tool_schema_format(value: &str) -> Option<ToolSchemaFormat> {
    match value.trim().to_ascii_lowercase().replace('-', "_").as_str() {
        "responses" => Some(ToolSchemaFormat::Responses),
        "chat_completions" => Some(ToolSchemaFormat::ChatCompletions),

        _ => None,
    }
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
                name: None,
                api_protocol: None,
                enabled: true,
                request: ModelRequestConfig::default(),
            },
        );
    }

    // Build a ProviderConfig with typed defaults from the schema.
    // Standard fields are mapped to their typed equivalents; everything
    // else goes into extension_fields.
    let mut config = ProviderConfig {
        kind: kind.to_string(),
        models,
        ..Default::default()
    };

    if let Some(properties) = config_schema.get("properties").and_then(Value::as_object) {
        for (key, schema_val) in properties {
            let default_val = match schema_val.get("default") {
                Some(val) if !val.is_null() => val.clone(),
                _ => continue,
            };
            // Map to typed fields using the same logic as normalize_for_key.
            // This keeps the two paths in sync.
            match key.as_str() {
                "credential" | "credentialEnv" => {
                    if config.credential_env.is_none() {
                        if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                            config.credential_env = Some(s.to_string());
                        }
                    }
                }
                "apiEndpoint" => {
                    if config.api_endpoint.is_empty() {
                        if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                            config.api_endpoint = s.trim_end_matches('/').to_string();
                        }
                    }
                }
                "httpHeaders" => {
                    if config.http_headers.is_empty() {
                        if let Some(obj) = default_val.as_object() {
                            config.http_headers = obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect();
                        }
                    }
                }
                "envHttpHeaders" => {
                    if config.env_http_headers.is_empty() {
                        if let Some(obj) = default_val.as_object() {
                            config.env_http_headers = obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect();
                        }
                    }
                }
                "queryParams" => {
                    if config.query_params.is_empty() {
                        if let Some(obj) = default_val.as_object() {
                            config.query_params = obj.iter()
                                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                                .collect();
                        }
                    }
                }
                "modelsEndpointCandidates" => {
                    if config.models_endpoint_candidates.is_empty() {
                        if let Some(arr) = default_val.as_array() {
                            config.models_endpoint_candidates = arr.iter()
                                .filter_map(|v| v.as_str().map(String::from))
                                .collect();
                        }
                    }
                }
                "realtimeEndpoint" => {
                    if config.realtime_endpoint.is_none() {
                        if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                            config.realtime_endpoint = Some(s.trim_end_matches('/').to_string());
                        }
                    }
                }
                "supportsWebsockets" => {
                    if let Some(b) = default_val.as_bool() {
                        config.supports_websockets = b;
                    }
                }
                "streamTransport" => {
                    if config.stream_transport == "sse" {
                        if let Some(s) = default_val.as_str().filter(|s| !s.is_empty()) {
                            config.stream_transport = s.to_string();
                        }
                    }
                }
                "requestTimeoutSeconds" => {
                    if config.request_timeout_seconds.is_none() {
                        config.request_timeout_seconds = default_val.as_u64();
                    }
                }
                "requestMaxRetries" => {
                    if config.request_max_retries.is_none() {
                        config.request_max_retries = default_val.as_u64().map(|v| v as u32);
                    }
                }
                "streamMaxRetries" => {
                    if config.stream_max_retries.is_none() {
                        config.stream_max_retries = default_val.as_u64().map(|v| v as u32);
                    }
                }
                "streamIdleTimeoutMs" => {
                    if config.stream_idle_timeout_ms.is_none() {
                        config.stream_idle_timeout_ms = default_val.as_u64();
                    }
                }
                "websocketConnectTimeoutMs" => {
                    if config.websocket_connect_timeout_ms.is_none() {
                        config.websocket_connect_timeout_ms = default_val.as_u64();
                    }
                }
                other => {
                    config.extension_fields.entry(other.to_string())
                        .or_insert(default_val);
                }
            }
        }
    }

    config
}
