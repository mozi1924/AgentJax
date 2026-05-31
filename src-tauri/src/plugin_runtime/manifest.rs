use super::SandboxPolicy;
use super::api::PLUGIN_API_VERSION;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashSet};

/// Tool declaration exported by a plugin.
///
/// This mirrors the shape we already use for native and MCP-backed tools so
/// plugin tools can be normalized into the same catalog later.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct PluginToolDefinition {
    pub name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub display_name: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default = "default_input_schema")]
    pub input_schema: Value,
    #[serde(default)]
    pub kind: PluginToolKind,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginToolKind {
    #[default]
    Function,
    Resource,
    Prompt,
}

/// Declarative provider definition exported by a model provider plugin.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginProviderDefinition {
    pub kind: String,
    pub display_name: String,
    #[serde(default = "default_config_schema")]
    pub config_schema: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub default_model_ids: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_priority: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_schema_format: Option<String>,
}

impl Default for PluginProviderDefinition {
    fn default() -> Self {
        Self {
            kind: String::new(),
            display_name: String::new(),
            config_schema: default_config_schema(),
            default_model_ids: Vec::new(),
            default_priority: None,
            capabilities: None,
            tool_schema_format: None,
        }
    }
}

fn default_config_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

/// Declarative plugin metadata that the host can validate before loading code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    #[serde(default = "default_api_version")]
    pub api_version: u32,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PluginToolDefinition>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub settings_sections: Vec<Value>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub settings_data: BTreeMap<String, Value>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub providers: Vec<PluginProviderDefinition>,
    #[serde(default)]
    pub sandbox: SandboxPolicy,
}

fn default_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

fn default_api_version() -> u32 {
    PLUGIN_API_VERSION
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            api_version: PLUGIN_API_VERSION,
            entrypoint: String::new(),
            description: String::new(),
            tools: Vec::new(),
            settings_sections: Vec::new(),
            settings_data: BTreeMap::new(),
            providers: Vec::new(),
            sandbox: SandboxPolicy::default(),
        }
    }
}

impl PluginManifest {
    /// Validate a manifest before the host accepts it into the registry.
    pub fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() {
            return Err("plugin id cannot be empty".to_string());
        }
        if self.name.trim().is_empty() {
            return Err(format!("plugin '{}' is missing a name", self.id));
        }
        if self.entrypoint.trim().is_empty() {
            return Err(format!("plugin '{}' is missing an entrypoint", self.id));
        }
        if self.api_version != PLUGIN_API_VERSION {
            return Err(format!(
                "plugin '{}' uses unsupported apiVersion {} (host supports {})",
                self.id, self.api_version, PLUGIN_API_VERSION
            ));
        }

        let mut tool_names = HashSet::new();
        for tool in &self.tools {
            let tool_name = tool.name.trim();
            if tool_name.is_empty() {
                return Err(format!(
                    "plugin '{}' exports a tool with an empty name",
                    self.id
                ));
            }
            if !tool_names.insert(tool_name.to_string()) {
                return Err(format!(
                    "plugin '{}' exports the tool '{}' more than once",
                    self.id, tool_name
                ));
            }
        }

        let mut provider_kinds = HashSet::new();
        for provider in &self.providers {
            let kind = provider.kind.trim().to_lowercase();
            if kind.is_empty() {
                return Err(format!(
                    "plugin '{}' exports a model provider with an empty kind",
                    self.id
                ));
            }
            if provider.display_name.trim().is_empty() {
                return Err(format!(
                    "plugin '{}' exports the model provider '{}' without a display name",
                    self.id, provider.kind
                ));
            }
            if !provider_kinds.insert(kind.clone()) {
                return Err(format!(
                    "plugin '{}' exports the model provider '{}' more than once",
                    self.id, provider.kind
                ));
            }
        }

        validate_settings_sections(&self.id, &self.settings_sections)?;

        Ok(())
    }
}

fn validate_settings_sections(plugin_id: &str, sections: &[Value]) -> Result<(), String> {
    let mut ids = HashSet::new();
    for section in sections {
        walk_settings_node(plugin_id, section, &mut ids)?;
    }
    Ok(())
}

fn walk_settings_node(
    plugin_id: &str,
    node: &Value,
    ids: &mut HashSet<String>,
) -> Result<(), String> {
    let object = node
        .as_object()
        .ok_or_else(|| format!("plugin '{plugin_id}' settings schema node must be an object"))?;
    let id = object
        .get("id")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            format!("plugin '{plugin_id}' settings schema node is missing a non-empty id")
        })?;
    if !ids.insert(id.to_string()) {
        return Err(format!(
            "plugin '{plugin_id}' settings schema contains duplicate node id '{id}'"
        ));
    }

    if let Some(children) = object.get("children") {
        let children = children.as_array().ok_or_else(|| {
            format!("plugin '{plugin_id}' settings schema node '{id}' children must be an array")
        })?;
        for child in children {
            walk_settings_node(plugin_id, child, ids)?;
        }
    }

    if let Some(tabs) = object.get("tabs") {
        let tabs = tabs.as_array().ok_or_else(|| {
            format!("plugin '{plugin_id}' settings schema node '{id}' tabs must be an array")
        })?;
        for tab in tabs {
            if let Some(children) = tab.get("children") {
                let children = children.as_array().ok_or_else(|| {
                    format!(
                        "plugin '{plugin_id}' settings schema tab in node '{id}' children must be an array"
                    )
                })?;
                for child in children {
                    walk_settings_node(plugin_id, child, ids)?;
                }
            }
        }
    }

    if let Some(item_template) = object.get("itemTemplate") {
        walk_settings_node(plugin_id, item_template, ids)?;
    }

    Ok(())
}
