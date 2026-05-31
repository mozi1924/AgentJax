use super::ToolCatalog;
use super::names::{mount_tool_name_for_server, prefixed_mcp_tool_name};
use super::schemas::{
    build_manage_mcp_server_tool_schema, normalize_mcp_tool_definitions,
};
use crate::plugin_runtime::{prefixed_plugin_tool_name, registered_tools_for_manifest};
use crate::tools::{ToolExecutionContext, ToolSchemaFormat, humanize_tool_name};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Optional query parameters for the read-only Tools Manager snapshot.
///
/// MCP tool discovery is intentionally source-scoped. A plain snapshot only
/// returns configured MCP sources and mounted metadata, avoiding incidental
/// process startup when the settings page opens.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ToolManagerSnapshotRequest {
    pub source_id: Option<String>,
    pub discover: bool,
    pub conversation_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolManagerSnapshot {
    pub sources: Vec<ToolManagerSourceSnapshot>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolManagerSourceSnapshot {
    pub source_type: ToolManagerSourceType,
    pub source_id: String,
    pub source_name: String,
    pub enabled: bool,
    pub status: String,
    pub exposure_mode: String,
    pub source_capabilities: Vec<String>,
    pub policy_paths: ToolManagerSourcePolicyPaths,
    pub tools: Vec<ToolManagerToolSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolManagerSourceType {
    Native,
    Mcp,
    Plugin,
    Dynamic,
    Control,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ToolManagerToolSnapshot {
    pub id: String,
    pub friendly_name: String,
    pub model_name: String,
    pub description: String,
    pub icon: Option<String>,
    pub enabled: bool,
    pub availability: String,
    pub schema_summary: ToolSchemaSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<Value>,
    pub schema_format: ToolManagerSchemaFormat,
    pub source_capabilities: Vec<String>,
    pub policy_paths: ToolManagerToolPolicyPaths,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolManagerSourcePolicyPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_enabled_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub exposure_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolManagerToolPolicyPaths {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_enabled_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ToolManagerSchemaFormat {
    JsonSchema,
    OpenaiFunction,
    Mcp,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ToolSchemaSummary {
    pub parameter_count: usize,
    pub required: Vec<String>,
    pub properties: Vec<String>,
}

impl ToolCatalog {
    pub async fn tool_manager_snapshot(
        &self,
        request: ToolManagerSnapshotRequest,
    ) -> ToolManagerSnapshot {
        let context = ToolExecutionContext {
            conversation_id: request
                .conversation_id
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
        };
        let discover_source_id = request
            .discover
            .then(|| request.source_id.as_deref().map(str::trim).unwrap_or(""))
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned);
        let mounted_servers = self.load_persisted_mounted_servers(&context);

        let mut sources = Vec::new();
        sources.push(self.native_tools_snapshot());
        sources.extend(
            self.mcp_sources_snapshot(&context, &mounted_servers, discover_source_id.as_deref())
                .await,
        );
        sources.extend(self.plugin_sources_snapshot());
        sources.push(self.dynamic_tools_snapshot(&context));
        sources.push(self.control_tools_snapshot(&mounted_servers));
        ToolManagerSnapshot { sources }
    }

    fn native_tools_snapshot(&self) -> ToolManagerSourceSnapshot {
        let tools = self
            .native_tools
            .iter()
            .map(|tool| {
                let enabled = self.native_tool_enabled(tool.name());
                let input_schema = tool.parameters_schema();
                ToolManagerToolSnapshot {
                    id: tool.name().to_string(),
                    friendly_name: tool.display_name().to_string(),
                    model_name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    icon: tool.icon().map(ToOwned::to_owned),
                    enabled,
                    availability: if enabled { "available" } else { "disabled" }.to_string(),
                    schema_summary: schema_summary(&input_schema),
                    input_schema: Some(input_schema),
                    schema_format: ToolManagerSchemaFormat::JsonSchema,
                    source_capabilities: vec!["policy:tool_enabled".to_string()],
                    policy_paths: ToolManagerToolPolicyPaths {
                        tool_enabled_path: Some(native_tool_enabled_path(tool.name())),
                    },
                }
            })
            .collect();

        ToolManagerSourceSnapshot {
            source_type: ToolManagerSourceType::Native,
            source_id: "native".to_string(),
            source_name: "Native Tools".to_string(),
            enabled: true,
            status: "ready".to_string(),
            exposure_mode: "always".to_string(),
            source_capabilities: vec!["policy:tool_enabled".to_string()],
            policy_paths: ToolManagerSourcePolicyPaths::default(),
            tools,
            error: None,
        }
    }

    async fn mcp_sources_snapshot(
        &self,
        context: &ToolExecutionContext,
        mounted_servers: &super::MountedToolSourceSessions,
        discover_source_id: Option<&str>,
    ) -> Vec<ToolManagerSourceSnapshot> {
        let mut sources = Vec::new();
        for (server_id, server_config) in &self.mcp_config {
            let source_policy_enabled = self.mcp_source_enabled(server_id);
            let source_enabled = server_config.enabled && source_policy_enabled;
            let exposure_mode = if self.mcp_source_unfolded(server_id, server_config) {
                "unfolded"
            } else {
                "collapsed"
            };
            let mounted = mounted_servers.get(server_id);
            let should_discover = discover_source_id == Some(server_id.as_str());
            let mut tools = mounted
                .map(|session| {
                    session
                        .tools
                        .iter()
                        .map(|tool| {
                            let enabled = self.mcp_tool_enabled(server_id, &tool.tool_name);
                            ToolManagerToolSnapshot {
                                id: tool.tool_name.clone(),
                                friendly_name: tool.display_name.clone(),
                                model_name: prefixed_mcp_tool_name(server_id, &tool.tool_name),
                                description: tool.description.clone(),
                                icon: tool.icon.clone(),
                                enabled,
                                availability: if enabled { "mounted" } else { "disabled" }
                                    .to_string(),
                                schema_summary: schema_summary(&tool.input_schema),
                                input_schema: Some(tool.input_schema.clone()),
                                schema_format: ToolManagerSchemaFormat::Mcp,
                                source_capabilities: mcp_source_capabilities(),
                                policy_paths: ToolManagerToolPolicyPaths {
                                    tool_enabled_path: Some(mcp_tool_enabled_path(
                                        server_id,
                                        &tool.tool_name,
                                    )),
                                },
                            }
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            let mut status = if !source_enabled {
                "disabled".to_string()
            } else if mounted.is_some() {
                "mounted".to_string()
            } else if exposure_mode == "unfolded" {
                "unfolded".to_string()
            } else {
                "configured".to_string()
            };
            let mut error = None;

            if should_discover && source_enabled {
                let resolved_config =
                    self.resolve_server_config_with_workspace_fallback(server_config, context);
                match self
                    .mcp_manager
                    .list_tools(server_id, &resolved_config, &self.mcp_runtime)
                    .await
                {
                    Ok(raw_tools) => {
                        status = "discovered".to_string();
                        tools = normalize_mcp_tool_definitions(raw_tools)
                            .into_iter()
                            .map(|tool| {
                                let enabled = self.mcp_tool_enabled(server_id, &tool.tool_name);
                                ToolManagerToolSnapshot {
                                    id: tool.tool_name.clone(),
                                    friendly_name: tool.display_name,
                                    model_name: prefixed_mcp_tool_name(server_id, &tool.tool_name),
                                    description: tool.description,
                                    icon: tool.icon,
                                    enabled,
                                    availability: if enabled { "available" } else { "disabled" }
                                        .to_string(),
                                    schema_summary: schema_summary(&tool.input_schema),
                                    input_schema: Some(tool.input_schema),
                                    schema_format: ToolManagerSchemaFormat::Mcp,
                                    source_capabilities: mcp_source_capabilities(),
                                    policy_paths: ToolManagerToolPolicyPaths {
                                        tool_enabled_path: Some(mcp_tool_enabled_path(
                                            server_id,
                                            &tool.tool_name,
                                        )),
                                    },
                                }
                            })
                            .collect();
                    }
                    Err(err) => {
                        status = "error".to_string();
                        error = Some(err);
                    }
                }
            }

            sources.push(ToolManagerSourceSnapshot {
                source_type: ToolManagerSourceType::Mcp,
                source_id: server_id.clone(),
                source_name: server_id.clone(),
                enabled: source_enabled,
                status,
                exposure_mode: exposure_mode.to_string(),
                source_capabilities: mcp_source_capabilities(),
                policy_paths: ToolManagerSourcePolicyPaths {
                    source_enabled_path: Some(mcp_source_enabled_path(server_id)),
                    exposure_path: Some(mcp_exposure_path(server_id)),
                },
                tools,
                error,
            });
        }
        sources
    }

    fn plugin_sources_snapshot(&self) -> Vec<ToolManagerSourceSnapshot> {
        self.plugin_manifests
            .values()
            .map(|manifest| {
                let source_enabled = self.plugin_source_enabled(&manifest.id);
                let tools = registered_tools_for_manifest(manifest)
                    .into_iter()
                    .map(|registered| {
                        let enabled =
                            self.plugin_tool_enabled(&registered.plugin_id, &registered.tool.name);
                        let friendly_name = if registered.tool.display_name.trim().is_empty() {
                            humanize_tool_name(&registered.tool.name)
                        } else {
                            registered.tool.display_name.clone()
                        };
                        ToolManagerToolSnapshot {
                            id: registered.tool.name.clone(),
                            friendly_name,
                            model_name: prefixed_plugin_tool_name(
                                &registered.plugin_id,
                                &registered.tool.name,
                            ),
                            description: registered.tool.description.clone(),
                            icon: registered.tool.icon.clone(),
                            enabled,
                            availability: if source_enabled && enabled {
                                "available"
                            } else {
                                "disabled"
                            }
                            .to_string(),
                            schema_summary: schema_summary(&registered.tool.input_schema),
                            input_schema: Some(registered.tool.input_schema.clone()),
                            schema_format: ToolManagerSchemaFormat::JsonSchema,
                            source_capabilities: plugin_source_capabilities(),
                            policy_paths: ToolManagerToolPolicyPaths {
                                tool_enabled_path: Some(plugin_tool_enabled_path(
                                    &registered.plugin_id,
                                    &registered.tool.name,
                                )),
                            },
                        }
                    })
                    .collect();

                ToolManagerSourceSnapshot {
                    source_type: ToolManagerSourceType::Plugin,
                    source_id: manifest.id.clone(),
                    source_name: if manifest.name.trim().is_empty() {
                        manifest.id.clone()
                    } else {
                        manifest.name.clone()
                    },
                    enabled: source_enabled,
                    status: if source_enabled { "ready" } else { "disabled" }.to_string(),
                    exposure_mode: "always".to_string(),
                    source_capabilities: plugin_source_capabilities(),
                    policy_paths: ToolManagerSourcePolicyPaths {
                        source_enabled_path: Some(plugin_source_enabled_path(&manifest.id)),
                        exposure_path: None,
                    },
                    tools,
                    error: None,
                }
            })
            .collect()
    }

    fn dynamic_tools_snapshot(&self, context: &ToolExecutionContext) -> ToolManagerSourceSnapshot {
        let tools = context
            .conversation_id
            .as_deref()
            .and_then(|conversation_id| {
                crate::conversation_store::load_conversation_dynamic_tools(conversation_id).ok()
            })
            .unwrap_or_default()
            .into_iter()
            .map(|tool| {
                let model_name = tool.name.clone();
                ToolManagerToolSnapshot {
                    id: tool.name,
                    friendly_name: tool
                        .display_name
                        .unwrap_or_else(|| humanize_tool_name(&model_name)),
                    model_name,
                    description: tool.description,
                    icon: tool.icon,
                    enabled: true,
                    availability: "session".to_string(),
                    schema_summary: schema_summary(&tool.parameters),
                    input_schema: Some(tool.parameters),
                    schema_format: ToolManagerSchemaFormat::JsonSchema,
                    source_capabilities: vec!["session".to_string()],
                    policy_paths: ToolManagerToolPolicyPaths::default(),
                }
            })
            .collect();

        ToolManagerSourceSnapshot {
            source_type: ToolManagerSourceType::Dynamic,
            source_id: context
                .conversation_id
                .clone()
                .unwrap_or_else(|| "current_session".to_string()),
            source_name: "Session Tools".to_string(),
            enabled: context.conversation_id.is_some(),
            status: if context.conversation_id.is_some() {
                "ready"
            } else {
                "no_session"
            }
            .to_string(),
            exposure_mode: "session".to_string(),
            source_capabilities: vec!["session".to_string()],
            policy_paths: ToolManagerSourcePolicyPaths::default(),
            tools,
            error: None,
        }
    }

    fn control_tools_snapshot(
        &self,
        mounted_servers: &super::MountedToolSourceSessions,
    ) -> ToolManagerSourceSnapshot {
        let tools = self
            .mcp_config
            .iter()
            .filter(|(server_id, server_config)| {
                server_config.enabled
                    && self.mcp_source_enabled(server_id)
                    && !self.mcp_source_unfolded(server_id, server_config)
            })
            .map(|(server_id, _)| {
                let model_name = mount_tool_name_for_server(server_id);
                let schema = build_manage_mcp_server_tool_schema(
                    ToolSchemaFormat::Responses,
                    server_id,
                    mounted_servers.contains_key(server_id),
                );
                let input_schema = schema_parameters(&schema).clone();
                ToolManagerToolSnapshot {
                    id: server_id.clone(),
                    friendly_name: format!("Manage {server_id}"),
                    model_name,
                    description: format!("Controls the MCP server '{server_id}'."),
                    icon: Some("Plug".to_string()),
                    enabled: true,
                    availability: "available".to_string(),
                    schema_summary: schema_summary(&input_schema),
                    input_schema: Some(input_schema),
                    schema_format: ToolManagerSchemaFormat::OpenaiFunction,
                    source_capabilities: vec!["mcp:control".to_string()],
                    policy_paths: ToolManagerToolPolicyPaths::default(),
                }
            })
            .collect();

        ToolManagerSourceSnapshot {
            source_type: ToolManagerSourceType::Control,
            source_id: "mcp_controls".to_string(),
            source_name: "MCP Controls".to_string(),
            enabled: true,
            status: "ready".to_string(),
            exposure_mode: "control".to_string(),
            source_capabilities: vec!["mcp:control".to_string()],
            policy_paths: ToolManagerSourcePolicyPaths::default(),
            tools,
            error: None,
        }
    }
}

fn schema_parameters(schema: &Value) -> &Value {
    schema
        .get("parameters")
        .or_else(|| schema.get("input_schema"))
        .or_else(|| {
            schema
                .get("function")
                .and_then(|function| function.get("parameters"))
        })
        .unwrap_or(&Value::Null)
}

fn schema_summary(schema: &Value) -> ToolSchemaSummary {
    let object = match schema.as_object() {
        Some(object) => object,
        None => return ToolSchemaSummary::default(),
    };
    let properties = object
        .get("properties")
        .and_then(Value::as_object)
        .map(|properties| properties.keys().cloned().collect::<Vec<_>>())
        .unwrap_or_default();
    let required = object
        .get("required")
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();

    ToolSchemaSummary {
        parameter_count: properties.len(),
        required,
        properties,
    }
}

fn escape_policy_path_segment(segment: &str) -> String {
    segment.replace('\\', "\\\\").replace('.', "\\.")
}

fn native_tool_enabled_path(tool_id: &str) -> String {
    format!(
        "tool_manager.native_tools.{}.enabled",
        escape_policy_path_segment(tool_id)
    )
}

fn plugin_source_enabled_path(source_id: &str) -> String {
    format!(
        "tool_manager.plugin_tools.{}.enabled",
        escape_policy_path_segment(source_id)
    )
}

fn plugin_tool_enabled_path(source_id: &str, tool_id: &str) -> String {
    format!(
        "tool_manager.plugin_tools.{}.tools.{}.enabled",
        escape_policy_path_segment(source_id),
        escape_policy_path_segment(tool_id)
    )
}

fn mcp_source_enabled_path(source_id: &str) -> String {
    format!(
        "tool_manager.mcp_tools.{}.enabled",
        escape_policy_path_segment(source_id)
    )
}

fn mcp_tool_enabled_path(source_id: &str, tool_id: &str) -> String {
    format!(
        "tool_manager.mcp_tools.{}.tools.{}.enabled",
        escape_policy_path_segment(source_id),
        escape_policy_path_segment(tool_id)
    )
}

fn mcp_exposure_path(source_id: &str) -> String {
    format!(
        "tool_manager.mcp_tools.{}.exposure",
        escape_policy_path_segment(source_id)
    )
}

fn plugin_source_capabilities() -> Vec<String> {
    vec![
        "policy:source_enabled".to_string(),
        "policy:tool_enabled".to_string(),
    ]
}

fn mcp_source_capabilities() -> Vec<String> {
    vec![
        "discover".to_string(),
        "policy:source_enabled".to_string(),
        "policy:tool_enabled".to_string(),
        "policy:exposure".to_string(),
    ]
}
