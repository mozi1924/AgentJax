use super::{AppConfig, SettingsOption};
use crate::error::AgentJaxResult;
use crate::models;
use crate::provider_api::registry;
use std::collections::BTreeMap;

// ── Trait ─────────────────────────────────────────────────────────────────────

/// A provider of dynamic settings options for the settings UI.
///
/// Each subsystem implements this trait to contribute its options to the
/// `dynamic_options` map that is sent to the frontend alongside the settings
/// snapshot.  The map keys must match `optionSourceKey` values in the settings
/// JSON schema files.
pub trait DynamicOptionsProvider: Send + Sync {
    /// Contribute options into the accumulating map.
    fn contribute(
        &self,
        config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()>;
}

// ── Option-scope helpers ──────────────────────────────────────────────────────

const OPTION_SCOPE_DELIMITER: &str = "@";

fn scoped_option_source(base_key: &str, context_path: &str) -> String {
    format!("{base_key}{OPTION_SCOPE_DELIMITER}{context_path}")
}

fn escape_path_segment(segment: &str) -> String {
    segment.replace('\\', "\\\\").replace('.', "\\.")
}

fn profile_key_from_ref(profile_ref: &str) -> String {
    profile_ref
        .split_once('/')
        .map(|(_, profile_key)| profile_key.to_string())
        .unwrap_or_else(|| profile_ref.to_string())
}

fn reasoning_options_with_default(levels: &[String]) -> Vec<SettingsOption> {
    let mut options = vec![SettingsOption {
        label: "Follow default".to_string(),
        value: "".to_string(),
    }];

    for level in levels {
        let normalized = level.trim().to_lowercase();
        if normalized.is_empty() || options.iter().any(|existing| existing.value == normalized) {
            continue;
        }
        options.push(SettingsOption {
            label: normalized.clone(),
            value: normalized,
        });
    }

    options
}

// ── Built-in providers ────────────────────────────────────────────────────────

/// Provider keys from the current configuration.
struct ProviderKeysProvider;

impl DynamicOptionsProvider for ProviderKeysProvider {
    fn contribute(
        &self,
        config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let provider_options = config
            .provider_keys()
            .into_iter()
            .map(|key| SettingsOption {
                label: key.clone(),
                value: key,
            })
            .collect();
        options.insert("provider_keys".to_string(), provider_options);
        Ok(())
    }
}

/// Model references from the current configuration.
struct ModelRefsProvider;

impl DynamicOptionsProvider for ModelRefsProvider {
    fn contribute(
        &self,
        config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let model_options: Vec<SettingsOption> = config
            .configured_models()
            .into_iter()
            .map(|model_ref| SettingsOption {
                label: model_ref.clone(),
                value: model_ref,
            })
            .collect();
        options.insert("model_refs".to_string(), model_options.clone());

        // Summarization model options: all model_refs + a "default" entry.
        let mut summarization_options = vec![SettingsOption {
            label: "settings.context_management.summarization_model.default".to_string(),
            value: String::new(), // empty = use utility_small_model
        }];
        summarization_options.extend(model_options);
        options.insert(
            "summarization_model_refs".to_string(),
            summarization_options,
        );

        Ok(())
    }
}

/// Provider kind options sourced from the provider registry.
struct ProviderKindProvider;

impl DynamicOptionsProvider for ProviderKindProvider {
    fn contribute(
        &self,
        _config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let kind_options: Vec<SettingsOption> = registry::provider_kind_options()
            .into_iter()
            .map(|(label, value)| SettingsOption { label, value })
            .collect();
        options.insert("provider_kind".to_string(), kind_options);
        Ok(())
    }
}

/// API protocol options sourced from the protocol registry.
struct ApiProtocolProvider;

impl DynamicOptionsProvider for ApiProtocolProvider {
    fn contribute(
        &self,
        _config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let protocol_options: Vec<SettingsOption> =
            crate::provider_api::protocol::builtin_protocols()
                .names()
                .map(|name| SettingsOption {
                    label: name.to_string(),
                    value: name.to_string(),
                })
                .collect();
        options.insert("api_protocol".to_string(), protocol_options);
        Ok(())
    }
}

/// Static stream transport options (websocket / sse).
struct StreamTransportProvider;

impl DynamicOptionsProvider for StreamTransportProvider {
    fn contribute(
        &self,
        _config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let transport_options: Vec<SettingsOption> = ["websocket", "sse"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect();
        options.insert("stream_transport".to_string(), transport_options);
        Ok(())
    }
}

/// Static MCP transport options (stdio / streamable_http).
struct McpTransportProvider;

impl DynamicOptionsProvider for McpTransportProvider {
    fn contribute(
        &self,
        _config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let transport_options: Vec<SettingsOption> = ["stdio", "streamable_http"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect();
        options.insert("mcp_transport".to_string(), transport_options);
        Ok(())
    }
}

/// Reasoning effort options, both per-model scoped and global.
struct ReasoningEffortProvider;

impl DynamicOptionsProvider for ReasoningEffortProvider {
    fn contribute(
        &self,
        config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        let reasoning_entries = models::get_model_catalog_entries_from_config(config)?;
        let mut global_reasoning_levels = Vec::new();

        for entry in &reasoning_entries {
            let context_path = format!(
                "providers.{}.models.{}",
                escape_path_segment(&entry.provider_key),
                escape_path_segment(&profile_key_from_ref(&entry.profile_key))
            );
            let levels = reasoning_options_with_default(&entry.supported_reasoning_levels);

            options.insert(
                scoped_option_source("reasoning_effort", &context_path),
                levels.clone(),
            );

            if levels.len() > 1 {
                for option in levels {
                    if option.value.is_empty()
                        || global_reasoning_levels
                            .iter()
                            .any(|existing: &SettingsOption| existing.value == option.value)
                    {
                        continue;
                    }
                    global_reasoning_levels.push(option);
                }
            }
        }

        let mut global_options = vec![SettingsOption {
            label: "Follow default".to_string(),
            value: "".to_string(),
        }];
        global_options.extend(global_reasoning_levels);
        options.insert("reasoning_effort".to_string(), global_options);

        Ok(())
    }
}

/// Street auto-trigger priority options (static).
struct StreetPrioritiesProvider;

impl DynamicOptionsProvider for StreetPrioritiesProvider {
    fn contribute(
        &self,
        _config: &AppConfig,
        options: &mut BTreeMap<String, Vec<SettingsOption>>,
    ) -> AgentJaxResult<()> {
        options.insert(
            "street_auto_trigger_priorities".to_string(),
            vec![
                SettingsOption {
                    label: "Never".to_string(),
                    value: "never".to_string(),
                },
                SettingsOption {
                    label: "Urgent only".to_string(),
                    value: "urgent".to_string(),
                },
                SettingsOption {
                    label: "High or above".to_string(),
                    value: "high".to_string(),
                },
                SettingsOption {
                    label: "Normal or above".to_string(),
                    value: "normal".to_string(),
                },
                SettingsOption {
                    label: "All (low or above)".to_string(),
                    value: "low".to_string(),
                },
            ],
        );
        Ok(())
    }
}

// ── Registry ──────────────────────────────────────────────────────────────────

/// Collect all registered dynamic option providers.
///
/// New providers should be registered here by adding them to the returned vec.
fn registered_providers() -> Vec<Box<dyn DynamicOptionsProvider>> {
    vec![
        Box::new(ProviderKeysProvider),
        Box::new(ModelRefsProvider),
        Box::new(ProviderKindProvider),
        Box::new(ApiProtocolProvider),
        Box::new(StreamTransportProvider),
        Box::new(McpTransportProvider),
        Box::new(ReasoningEffortProvider),
        Box::new(StreetPrioritiesProvider),
    ]
}

// ── Public API ────────────────────────────────────────────────────────────────

/// Build the full dynamic options map by delegating to all registered providers.
pub fn build_dynamic_options(config: &AppConfig) -> AgentJaxResult<BTreeMap<String, Vec<SettingsOption>>> {
    let providers = registered_providers();
    let mut options = BTreeMap::new();
    for provider in &providers {
        provider.contribute(config, &mut options)?;
    }
    Ok(options)
}
