use super::{AppConfig, SettingsOption};
use crate::models;
use crate::plugin_runtime::{PluginPackage, discover_all_plugin_packages};
use crate::provider_api::registry;
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;

const OPTION_SCOPE_DELIMITER: &str = "@";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsUiSnapshot {
    pub snapshot: super::SettingsSnapshot,
    pub sections: Vec<Value>,
}

const GENERAL_SECTION_JSON: &str = include_str!("settings_ui_sections/general.json");
const PROMPT_COMPOSER_SECTION_JSON: &str =
    include_str!("settings_ui_sections/prompt_composer.json");
const PROVIDERS_SECTION_JSON: &str = include_str!("settings_ui_sections/providers.json");
const MODEL_PROFILES_SECTION_JSON: &str = include_str!("settings_ui_sections/model_profiles.json");
const MCP_RUNTIME_SECTION_JSON: &str = include_str!("settings_ui_sections/mcp_runtime.json");
const MCP_SERVERS_SECTION_JSON: &str = include_str!("settings_ui_sections/mcp_servers.json");
const TOOLS_SECTION_JSON: &str = include_str!("settings_ui_sections/tools.json");
const PLUGIN_MANAGER_SECTION_JSON: &str =
    include_str!("settings_ui_sections/plugin_manager.json");
const LCM_SECTION_JSON: &str = include_str!("settings_ui_sections/lcm.json");

pub fn build_settings_sections() -> Result<Vec<Value>, String> {
    let mut sections = build_builtin_settings_sections()?;
    match discover_all_plugin_packages() {
        Ok(packages) => sections.extend(plugin_settings_sections_from_packages(packages)),
        Err(err) => {
            log::warn!("Failed to discover plugin settings sections: {err}");
        }
    }

    validate_unique_schema_ids(&sections)?;
    Ok(sections)
}

fn build_builtin_settings_sections() -> Result<Vec<Value>, String> {
    let section_sources = [
        GENERAL_SECTION_JSON,
        PROMPT_COMPOSER_SECTION_JSON,
        PROVIDERS_SECTION_JSON,
        MODEL_PROFILES_SECTION_JSON,
        MCP_RUNTIME_SECTION_JSON,
        MCP_SERVERS_SECTION_JSON,
        TOOLS_SECTION_JSON,
        PLUGIN_MANAGER_SECTION_JSON,
        LCM_SECTION_JSON,
    ];

    let mut sections = Vec::with_capacity(section_sources.len());
    for source in section_sources {
        let section: Value = serde_json::from_str(source)
            .map_err(|error| format!("Failed to parse settings section JSON: {error}"))?;
        sections.push(section);
    }

    Ok(sections)
}

fn plugin_settings_sections_from_packages(
    packages: impl IntoIterator<Item = PluginPackage>,
) -> Vec<Value> {
    packages
        .into_iter()
        .flat_map(|package| package.manifest.settings_sections)
        .collect()
}

fn scoped_option_source(base_key: &str, context_path: &str) -> String {
    format!("{base_key}{OPTION_SCOPE_DELIMITER}{context_path}")
}

fn escape_path_segment(segment: &str) -> String {
    segment.replace('\\', "\\\\").replace('.', "\\.")
}

pub fn build_dynamic_options(
    config: &AppConfig,
) -> Result<BTreeMap<String, Vec<SettingsOption>>, String> {
    let mut dynamic_options = BTreeMap::new();

    let provider_options = config
        .provider_keys()
        .into_iter()
        .map(|provider_key| SettingsOption {
            label: provider_key.clone(),
            value: provider_key,
        })
        .collect::<Vec<_>>();
    dynamic_options.insert("provider_keys".to_string(), provider_options);

    let model_options = config
        .configured_models()
        .into_iter()
        .map(|model_ref| SettingsOption {
            label: model_ref.clone(),
            value: model_ref,
        })
        .collect::<Vec<_>>();
    dynamic_options.insert("model_refs".to_string(), model_options);

    // Summarization model options: all model_refs + a "default" entry.
    let mut summarization_model_options: Vec<SettingsOption> = vec![SettingsOption {
        label: "settings.lcm.summarization_model.default".to_string(),
        value: String::new(), // empty = use utility_small_model
    }];
    summarization_model_options.extend(config.configured_models().into_iter().map(|model_ref| {
        SettingsOption {
            label: model_ref.clone(),
            value: model_ref,
        }
    }));
    dynamic_options.insert(
        "summarization_model_refs".to_string(),
        summarization_model_options,
    );

    dynamic_options.insert(
        "provider_kind".to_string(),
        registry::provider_kind_options()
            .into_iter()
            .map(|(label, value)| SettingsOption { label, value })
            .collect(),
    );
    dynamic_options.insert(
        "stream_transport".to_string(),
        ["websocket", "sse"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect(),
    );
    dynamic_options.insert(
        "mcp_transport".to_string(),
        ["stdio", "streamable_http"]
            .into_iter()
            .map(|entry| SettingsOption {
                label: entry.to_string(),
                value: entry.to_string(),
            })
            .collect(),
    );

    let reasoning_entries = models::get_model_catalog_entries_from_config(config)?;
    let mut global_reasoning_levels = Vec::new();

    for entry in reasoning_entries {
        let context_path = format!(
            "providers.{}.models.{}",
            escape_path_segment(&entry.provider_key),
            escape_path_segment(&profile_key_from_ref(&entry.profile_key))
        );
        let options = reasoning_options_with_default(&entry.supported_reasoning_levels);

        dynamic_options.insert(
            scoped_option_source("reasoning_effort", &context_path),
            options.clone(),
        );

        if options.len() > 1 {
            for option in options {
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
    dynamic_options.insert("reasoning_effort".to_string(), global_options);

    Ok(dynamic_options)
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

fn profile_key_from_ref(profile_ref: &str) -> String {
    profile_ref
        .split_once('/')
        .map(|(_, profile_key)| profile_key.to_string())
        .unwrap_or_else(|| profile_ref.to_string())
}

fn validate_unique_schema_ids(sections: &[Value]) -> Result<(), String> {
    let mut ids = std::collections::BTreeSet::new();
    for section in sections {
        walk_node_ids(section, &mut ids)?;
    }
    Ok(())
}

fn walk_node_ids(node: &Value, ids: &mut std::collections::BTreeSet<String>) -> Result<(), String> {
    let object = node
        .as_object()
        .ok_or_else(|| "Settings schema node must be an object".to_string())?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "Settings schema node is missing non-empty id".to_string())?;

    if !ids.insert(id.to_string()) {
        return Err(format!("Duplicate settings schema node id: {id}"));
    }

    if let Some(children) = object.get("children") {
        let array = children
            .as_array()
            .ok_or_else(|| format!("Settings schema node '{id}' children must be an array"))?;
        for child in array {
            walk_node_ids(child, ids)?;
        }
    }

    if let Some(tabs) = object.get("tabs") {
        let array = tabs
            .as_array()
            .ok_or_else(|| format!("Settings schema node '{id}' tabs must be an array"))?;
        for tab in array {
            if let Some(children) = tab.get("children") {
                let children = children.as_array().ok_or_else(|| {
                    format!("Settings schema tab in node '{id}' children must be an array")
                })?;
                for child in children {
                    walk_node_ids(child, ids)?;
                }
            }
        }
    }

    if let Some(item_template) = object.get("itemTemplate") {
        walk_node_ids(item_template, ids)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::{PLUGIN_API_VERSION, PluginManifest};
    use std::path::PathBuf;

    fn package_with_settings(id: &str, sections: Vec<Value>) -> PluginPackage {
        PluginPackage {
            manifest: PluginManifest {
                id: id.to_string(),
                name: id.to_string(),
                version: "0.1.0".to_string(),
                api_version: PLUGIN_API_VERSION,
                entrypoint: "plugin.js".to_string(),
                description: String::new(),
                tools: Vec::new(),
                settings_sections: sections,
                settings_data: Default::default(),
                providers: Vec::new(),
                sandbox: Default::default(),
            },
            root_dir: PathBuf::from("/tmp/plugin"),
            manifest_path: PathBuf::from("/tmp/plugin/plugin.json"),
            entrypoint_source: None,
            is_builtin: false,
        }
    }

    #[test]
    fn plugin_settings_sections_are_appended_to_settings_schema() {
        let sections = plugin_settings_sections_from_packages([package_with_settings(
            "plugin.demo",
            vec![serde_json::json!({
                "id": "plugin.demo.settings",
                "title": "Demo",
                "icon": "Puzzle",
                "order": 900,
                "children": [{
                    "kind": "collapsible",
                    "id": "plugin.demo.settings.advanced",
                    "title": "Advanced",
                    "defaultExpanded": false,
                    "children": []
                }]
            })],
        )]);

        assert_eq!(sections.len(), 1);
        assert_eq!(sections[0]["id"], "plugin.demo.settings");
        validate_unique_schema_ids(&sections).expect("plugin settings ids should validate");
    }

    #[test]
    fn settings_schema_id_validation_walks_tabs_and_item_templates() {
        let sections = vec![serde_json::json!({
            "id": "plugin.demo.settings",
            "title": "Demo",
            "icon": "Puzzle",
            "order": 900,
            "children": [{
                "kind": "tabs",
                "id": "plugin.demo.tabs",
                "tabs": [{
                    "id": "general",
                    "title": "General",
                    "children": [{
                        "kind": "list",
                        "id": "plugin.demo.list",
                        "itemTemplate": {
                            "kind": "detail",
                            "id": "plugin.demo.list.item"
                        }
                    }]
                }]
            }]
        })];

        validate_unique_schema_ids(&sections).expect("nested ids should validate");
    }
}
