use super::SandboxPolicy;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::HashSet;

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

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PluginToolKind {
    #[default]
    Function,
    Resource,
    Prompt,
}

/// Declarative plugin metadata that the host can validate before loading code.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct PluginManifest {
    pub id: String,
    pub name: String,
    pub version: String,
    pub entrypoint: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tools: Vec<PluginToolDefinition>,
    #[serde(default)]
    pub sandbox: SandboxPolicy,
}

fn default_input_schema() -> Value {
    serde_json::json!({
        "type": "object",
        "properties": {}
    })
}

impl Default for PluginManifest {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            version: String::new(),
            entrypoint: String::new(),
            description: String::new(),
            tools: Vec::new(),
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

        Ok(())
    }
}
