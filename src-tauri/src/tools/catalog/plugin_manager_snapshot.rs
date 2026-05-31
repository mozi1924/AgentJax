use crate::config::{PluginEntryConfig, PluginManagerConfig};
use crate::plugin_runtime::PluginPackage;
use serde::Serialize;
use std::collections::BTreeMap;

/// Snapshot of all plugins for the Plugin Manager UI.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginManagerSnapshot {
    pub plugins: Vec<PluginEntrySnapshot>,
}

/// A single plugin entry in the Plugin Manager.
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntrySnapshot {
    /// Plugin identifier (e.g. "agentjax.provider.anthropic")
    pub id: String,
    /// Human-readable plugin name
    pub name: String,
    /// Plugin version
    pub version: String,
    /// Plugin description
    pub description: String,
    /// Whether this plugin is compiled into the binary
    pub is_builtin: bool,
    /// Whether the plugin is enabled at the plugin-manager level
    pub enabled: bool,
    /// Whether the plugin registers any tools
    pub has_tools: bool,
    /// Sandbox permissions declared in the manifest
    pub declared_permissions: DeclaredPermissions,
    /// Effective permissions after applying user overrides
    pub effective_permissions: EffectivePermissions,
    /// Config paths for policy patching
    pub policy_paths: PluginEntryPolicyPaths,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct DeclaredPermissions {
    pub allow_network: bool,
    pub allow_file_read: bool,
    pub allow_file_write: bool,
    pub allow_process_spawn: bool,
    pub allow_env_read: bool,
    pub allowed_hosts: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct EffectivePermissions {
    pub allow_network: bool,
    pub allow_file_read: bool,
    pub allow_file_write: bool,
    pub allow_process_spawn: bool,
    pub allow_env_read: bool,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginEntryPolicyPaths {
    pub plugin_enabled_path: String,
    pub permission_network_path: String,
    pub permission_file_read_path: String,
    pub permission_file_write_path: String,
    pub permission_process_spawn_path: String,
    pub permission_env_read_path: String,
}

/// Build the plugin manager snapshot from discovered packages and the current config.
pub fn build_plugin_manager_snapshot(
    packages: &BTreeMap<String, PluginPackage>,
    plugin_manager: &PluginManagerConfig,
) -> PluginManagerSnapshot {
    let mut plugins: Vec<PluginEntrySnapshot> = packages
        .values()
        .map(|package| build_plugin_entry(package, plugin_manager))
        .collect();
    plugins.sort_by(|a, b| {
        if a.is_builtin != b.is_builtin {
            // Built-in plugins first
            if a.is_builtin {
                std::cmp::Ordering::Less
            } else {
                std::cmp::Ordering::Greater
            }
        } else {
            a.id.cmp(&b.id)
        }
    });
    PluginManagerSnapshot { plugins }
}

fn build_plugin_entry(
    package: &PluginPackage,
    plugin_manager: &PluginManagerConfig,
) -> PluginEntrySnapshot {
    let manifest = &package.manifest;
    let entry_config: Option<&PluginEntryConfig> =
        plugin_manager.plugins.get(&manifest.id.to_ascii_lowercase());
    let enabled = entry_config.map(|e| e.enabled).unwrap_or(true);

    let sandbox = &manifest.sandbox;
    let declared = DeclaredPermissions {
        allow_network: sandbox.allow_network,
        allow_file_read: sandbox.allow_file_read,
        allow_file_write: sandbox.allow_file_write,
        allow_process_spawn: sandbox.allow_process_spawn,
        allow_env_read: sandbox.allow_env_read,
        allowed_hosts: sandbox.allowed_hosts.clone(),
    };

    // Effective permissions: use declared values as base, then overlay any
    // user-configured overrides (per-field `Option<bool>`).
    let effective = EffectivePermissions {
        allow_network: entry_config
            .and_then(|e| e.permissions.as_ref())
            .and_then(|p| p.allow_network)
            .unwrap_or(declared.allow_network),
        allow_file_read: entry_config
            .and_then(|e| e.permissions.as_ref())
            .and_then(|p| p.allow_file_read)
            .unwrap_or(declared.allow_file_read),
        allow_file_write: entry_config
            .and_then(|e| e.permissions.as_ref())
            .and_then(|p| p.allow_file_write)
            .unwrap_or(declared.allow_file_write),
        allow_process_spawn: entry_config
            .and_then(|e| e.permissions.as_ref())
            .and_then(|p| p.allow_process_spawn)
            .unwrap_or(declared.allow_process_spawn),
        allow_env_read: entry_config
            .and_then(|e| e.permissions.as_ref())
            .and_then(|p| p.allow_env_read)
            .unwrap_or(declared.allow_env_read),
    };

    let key = &manifest.id.to_ascii_lowercase();
    let escaped_key = escape_policy_segment(key);

    PluginEntrySnapshot {
        id: manifest.id.clone(),
        name: manifest.name.clone(),
        version: manifest.version.clone(),
        description: manifest.description.clone(),
        is_builtin: package.is_builtin,
        enabled,
        has_tools: !manifest.tools.is_empty(),
        declared_permissions: declared,
        effective_permissions: effective,
        policy_paths: PluginEntryPolicyPaths {
            plugin_enabled_path: format!("plugin_manager.plugins.{escaped_key}.enabled"),
            permission_network_path: format!(
                "plugin_manager.plugins.{escaped_key}.permissions.allow_network"
            ),
            permission_file_read_path: format!(
                "plugin_manager.plugins.{escaped_key}.permissions.allow_file_read"
            ),
            permission_file_write_path: format!(
                "plugin_manager.plugins.{escaped_key}.permissions.allow_file_write"
            ),
            permission_process_spawn_path: format!(
                "plugin_manager.plugins.{escaped_key}.permissions.allow_process_spawn"
            ),
            permission_env_read_path: format!(
                "plugin_manager.plugins.{escaped_key}.permissions.allow_env_read"
            ),
        },
    }
}

fn escape_policy_segment(segment: &str) -> String {
    segment.replace('\\', "\\\\").replace('.', "\\.")
}
