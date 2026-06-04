use crate::agentjax_err;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::plugin_runtime::{PluginPackage, discover_all_plugin_packages};
use serde::Serialize;
use serde_json::Value;

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
const MCP_SECTION_JSON: &str = include_str!("settings_ui_sections/mcp.json");
const TOOLS_SECTION_JSON: &str = include_str!("settings_ui_sections/tools.json");
const PLUGIN_MANAGER_SECTION_JSON: &str =
    include_str!("settings_ui_sections/plugin_manager.json");
const CONTEXT_MANAGEMENT_SECTION_JSON: &str =
    include_str!("settings_ui_sections/context_management.json");
const MEMORY_SECTION_JSON: &str = include_str!("settings_ui_sections/memory.json");

pub fn build_settings_sections() -> AgentJaxResult<Vec<Value>> {
    let mut sections = build_builtin_settings_sections()?;
    match discover_all_plugin_packages() {
        Ok(packages) => sections.extend(plugin_settings_sections_from_packages(packages)),
        Err(err) => {
            log::warn!("Failed to discover plugin settings sections: {err}");
        }
    }

    sections.sort_by_key(|v| {
        v.get("order")
            .and_then(Value::as_i64)
            .unwrap_or(1000)
    });

    validate_unique_schema_ids(&sections)?;
    Ok(sections)
}

fn build_builtin_settings_sections() -> AgentJaxResult<Vec<Value>> {
    let section_sources = [
        GENERAL_SECTION_JSON,
        PROMPT_COMPOSER_SECTION_JSON,
        PROVIDERS_SECTION_JSON,
        MCP_SECTION_JSON,
        TOOLS_SECTION_JSON,
        PLUGIN_MANAGER_SECTION_JSON,
        CONTEXT_MANAGEMENT_SECTION_JSON,
        MEMORY_SECTION_JSON,
    ];

    let mut sections = Vec::with_capacity(section_sources.len());
    for source in section_sources {
        let section: Value = serde_json::from_str(source)
            .map_err(|error| AgentJaxError::config(format!("Failed to parse settings section JSON: {error}")).with_error_source(&error))?;
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

fn validate_unique_schema_ids(sections: &[Value]) -> AgentJaxResult<()> {
    let mut ids = std::collections::BTreeSet::new();
    for section in sections {
        walk_node_ids(section, &mut ids)?;
    }
    Ok(())
}

fn walk_node_ids(node: &Value, ids: &mut std::collections::BTreeSet<String>) -> AgentJaxResult<()> {
    let object = node
        .as_object()
        .ok_or_else(|| agentjax_err!("Settings schema node must be an object", Config))?;

    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| agentjax_err!("Settings schema node is missing non-empty id", Config))?;

    if !ids.insert(id.to_string()) {
        return Err(agentjax_err!(format!("Duplicate settings schema node id: {id}"), Config));
    }

    if let Some(children) = object.get("children") {
        let array = children
            .as_array()
            .ok_or_else(|| agentjax_err!(format!("Settings schema node '{id}' children must be an array"), Config))?;
        for child in array {
            walk_node_ids(child, ids)?;
        }
    }

    if let Some(tabs) = object.get("tabs") {
        let array = tabs
            .as_array()
            .ok_or_else(|| agentjax_err!(format!("Settings schema node '{id}' tabs must be an array"), Config))?;
        for tab in array {
            if let Some(children) = tab.get("children") {
                let children = children.as_array().ok_or_else(|| {
                    agentjax_err!(format!("Settings schema tab in node '{id}' children must be an array"), Config)
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
    use crate::plugin_runtime::api::PLUGIN_API_VERSION;
    use crate::plugin_runtime::PluginManifest;
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
