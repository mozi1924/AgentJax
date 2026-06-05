use crate::config;
use crate::plugin_runtime::{PluginPackage, discover_all_plugin_packages};
use crate::tools::{
    PluginManagerSnapshot, ToolCatalog, ToolManagerSnapshot, ToolManagerSnapshotRequest,
    build_plugin_manager_snapshot,
};
use serde::Serialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn get_tool_manager_snapshot(
    mcp_manager: State<'_, Arc<crate::mcp::McpManager>>,
    request: Option<ToolManagerSnapshotRequest>,
) -> Result<ToolManagerSnapshot, String> {
    let config = config::load_config()?;
    let agent_config = config::AgentConfig::default();
    let catalog =
        ToolCatalog::new_with_home_plugins(mcp_manager.inner().clone(), &config, &agent_config);
    Ok(catalog
        .tool_manager_snapshot(request.unwrap_or_default())
        .await)
}

/// Snapshot of all discovered plugins for the Plugin Manager UI.
#[tauri::command]
pub fn get_plugin_manager_snapshot() -> Result<PluginManagerSnapshot, String> {
    let config = config::load_config()?;
    let packages = discover_all_plugin_packages().map_err(|err| err.to_string())?;
    let package_map: BTreeMap<String, PluginPackage> = packages
        .into_iter()
        .map(|pkg| (pkg.manifest.id.clone(), pkg))
        .collect();
    Ok(build_plugin_manager_snapshot(
        &package_map,
        &config.plugin_manager,
    ))
}

/// Snapshot of plugin-owned SchemaRenderer data sources.
///
/// `settingsSections` describe the UI shape, while `settingsData` gives simple
/// manifest-backed data for lists, details, and generic plugin settings panels.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginSettingsSnapshot {
    pub data_sources: BTreeMap<String, Value>,
}

#[tauri::command]
pub fn get_plugin_settings_snapshot() -> Result<PluginSettingsSnapshot, String> {
    let packages = discover_all_plugin_packages().map_err(|err| err.to_string())?;
    plugin_settings_snapshot_from_packages(packages)
}

pub(crate) fn plugin_settings_snapshot_from_packages(
    packages: impl IntoIterator<Item = PluginPackage>,
) -> Result<PluginSettingsSnapshot, String> {
    let packages = packages.into_iter().collect::<Vec<_>>();
    let mut data_sources = BTreeMap::new();

    let plugin_entries = packages
        .iter()
        .map(|package| {
            serde_json::json!({
                "id": package.manifest.id,
                "name": package.manifest.name,
                "version": package.manifest.version,
                "description": package.manifest.description,
            })
        })
        .collect::<Vec<_>>();
    data_sources.insert("plugin.plugins".to_string(), Value::Array(plugin_entries));

    for package in packages {
        for (key, value) in package.manifest.settings_data {
            let data_source = normalize_plugin_settings_data_key(&package.manifest.id, &key)?;
            if data_sources.insert(data_source.clone(), value).is_some() {
                return Err(format!(
                    "Duplicate plugin settings data source '{data_source}'"
                ));
            }
        }
    }

    Ok(PluginSettingsSnapshot { data_sources })
}

fn normalize_plugin_settings_data_key(plugin_id: &str, key: &str) -> Result<String, String> {
    let key = key.trim();
    if key.is_empty() {
        return Err(format!(
            "Plugin '{plugin_id}' settingsData contains an empty data source key"
        ));
    }
    if key == "plugin" || key.starts_with("plugin.") {
        return Ok(key.to_string());
    }
    Ok(format!("plugin.{plugin_id}.{key}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugin_runtime::api::PLUGIN_API_VERSION;
    use crate::plugin_runtime::{PluginManifest, SandboxPolicy};
    use std::path::PathBuf;

    fn package_with_settings_data(
        plugin_id: &str,
        settings_data: BTreeMap<String, Value>,
    ) -> PluginPackage {
        PluginPackage {
            manifest: PluginManifest {
                id: plugin_id.to_string(),
                name: plugin_id.to_string(),
                version: "0.1.0".to_string(),
                api_version: PLUGIN_API_VERSION,
                entrypoint: "plugin.js".to_string(),
                description: "Demo plugin".to_string(),
                tools: Vec::new(),
                settings_sections: Vec::new(),
                settings_data,
                providers: Vec::new(),
                sandbox: SandboxPolicy::default(),
            },
            root_dir: PathBuf::from("/tmp/plugin"),
            manifest_path: PathBuf::from("/tmp/plugin/plugin.json"),
            entrypoint_source: None,
            is_builtin: false,
        }
    }

    #[test]
    fn plugin_settings_snapshot_materializes_manifest_data_sources() {
        let snapshot = plugin_settings_snapshot_from_packages([
            package_with_settings_data(
                "local.demo",
                BTreeMap::from([(
                    "items".to_string(),
                    serde_json::json!([{ "id": "first", "name": "First" }]),
                )]),
            ),
            package_with_settings_data(
                "local.explicit",
                BTreeMap::from([(
                    "plugin.shared.items".to_string(),
                    serde_json::json!([{ "id": "shared" }]),
                )]),
            ),
        ])
        .expect("plugin settings snapshot");

        assert_eq!(
            snapshot.data_sources["plugin.plugins"][0]["id"],
            "local.demo"
        );
        assert_eq!(
            snapshot.data_sources["plugin.local.demo.items"][0]["name"],
            "First"
        );
        assert_eq!(
            snapshot.data_sources["plugin.shared.items"][0]["id"],
            "shared"
        );
    }

    #[test]
    fn plugin_settings_snapshot_rejects_duplicate_data_sources() {
        let result = plugin_settings_snapshot_from_packages([
            package_with_settings_data(
                "local.one",
                BTreeMap::from([("plugin.shared.items".to_string(), serde_json::json!([]))]),
            ),
            package_with_settings_data(
                "local.two",
                BTreeMap::from([("plugin.shared.items".to_string(), serde_json::json!([]))]),
            ),
        ]);

        assert!(result.is_err());
    }
}
