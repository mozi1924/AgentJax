use super::ToolCatalog;
use super::names::{mount_tool_name_for_server, prefixed_mcp_tool_name};
use super::schemas::{
    CANCEL_BACKGROUND_TOOL_NAME, LIST_BACKGROUND_TOOLS_NAME, START_BACKGROUND_TOOL_NAME,
    WAIT_BACKGROUND_TOOL_NAME, build_cancel_background_tool_schema,
    build_list_background_tools_schema, build_manage_mcp_server_tool_schema,
    build_start_background_tool_schema, build_wait_background_tool_schema,
    normalize_mcp_tool_definitions,
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
    Background,
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
        sources.push(self.background_tools_snapshot());
        sources.push(self.control_tools_snapshot(&mounted_servers));
        ToolManagerSnapshot { sources }
    }

    fn native_tools_snapshot(&self) -> ToolManagerSourceSnapshot {
        let tools = self
            .native_tools
            .iter()
            .map(|tool| {
                let enabled = self.native_tool_enabled(tool.name());
                ToolManagerToolSnapshot {
                    id: tool.name().to_string(),
                    friendly_name: tool.display_name().to_string(),
                    model_name: tool.name().to_string(),
                    description: tool.description().to_string(),
                    icon: tool.icon().map(ToOwned::to_owned),
                    enabled,
                    availability: if enabled { "available" } else { "disabled" }.to_string(),
                    schema_summary: schema_summary(&tool.parameters_schema()),
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
            tools,
            error: None,
        }
    }

    fn background_tools_snapshot(&self) -> ToolManagerSourceSnapshot {
        let tools = [
            (
                START_BACKGROUND_TOOL_NAME,
                "Start Background Tool",
                "Starts a tool as a background job.",
                Some("Rocket".to_string()),
                build_start_background_tool_schema(ToolSchemaFormat::Responses),
            ),
            (
                WAIT_BACKGROUND_TOOL_NAME,
                "Wait Background Tool",
                "Waits for a background tool job.",
                Some("Timer".to_string()),
                build_wait_background_tool_schema(ToolSchemaFormat::Responses),
            ),
            (
                CANCEL_BACKGROUND_TOOL_NAME,
                "Cancel Background Tool",
                "Cancels a background tool job.",
                Some("CircleStop".to_string()),
                build_cancel_background_tool_schema(ToolSchemaFormat::Responses),
            ),
            (
                LIST_BACKGROUND_TOOLS_NAME,
                "List Background Tools",
                "Lists background tool jobs.",
                Some("ListChecks".to_string()),
                build_list_background_tools_schema(ToolSchemaFormat::Responses),
            ),
        ]
        .into_iter()
        .map(
            |(id, friendly_name, description, icon, schema)| ToolManagerToolSnapshot {
                id: id.to_string(),
                friendly_name: friendly_name.to_string(),
                model_name: id.to_string(),
                description: description.to_string(),
                icon,
                enabled: true,
                availability: "available".to_string(),
                schema_summary: schema_summary(schema_parameters(&schema)),
            },
        )
        .collect();

        ToolManagerSourceSnapshot {
            source_type: ToolManagerSourceType::Background,
            source_id: "background".to_string(),
            source_name: "Background Tools".to_string(),
            enabled: true,
            status: "ready".to_string(),
            exposure_mode: "control".to_string(),
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
                ToolManagerToolSnapshot {
                    id: server_id.clone(),
                    friendly_name: format!("Manage {server_id}"),
                    model_name,
                    description: format!("Controls the MCP server '{server_id}'."),
                    icon: Some("Plug".to_string()),
                    enabled: true,
                    availability: "available".to_string(),
                    schema_summary: schema_summary(schema_parameters(&schema)),
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
