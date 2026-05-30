mod background;
mod dynamic;
mod mounted;
mod names;
mod plugin_execution;
mod schemas;
mod snapshot;
mod types;

use crate::plugin_runtime::{
    DenoCorePluginRuntime, PluginManifest, PluginPackage, SandboxPolicy,
    discover_home_plugin_packages, prefixed_plugin_tool_name, registered_tools_for_manifest,
};
use crate::tools::{
    CalculatorTool, EditFileTool, FileReaderTool, FileWriterTool, ListFilesTool, MkdirTool,
    SystemTimeTool, Tool, ToolExecutionContext, ToolPresentation, ToolSchemaFormat,
    format_tool_schema, humanize_tool_name,
};
use names::{
    mount_tool_name_for_server, prefixed_mcp_tool_name, presentation_for_manage_mcp_server,
};
use schemas::{
    CANCEL_BACKGROUND_TOOL_NAME, LIST_BACKGROUND_TOOLS_NAME, START_BACKGROUND_TOOL_NAME,
    WAIT_BACKGROUND_TOOL_NAME, build_cancel_background_tool_schema,
    build_list_background_tools_schema, build_manage_mcp_server_tool_schema,
    build_start_background_tool_schema, build_wait_background_tool_schema,
    normalize_mcp_tool_definitions,
};
use serde_json::Value;
pub use snapshot::ToolCatalogSnapshot;
use snapshot::{ToolSnapshotEntry, insert_snapshot_tool};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
pub(crate) use types::ToolCatalogExecution;
pub use types::{
    MountedToolDefinition, MountedToolSourceSession, MountedToolSourceSessions,
    ToolCatalogStateChange,
};

pub struct ToolCatalog {
    native_tools: Vec<Arc<dyn Tool>>,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_runtime: crate::config::McpRuntimeConfig,
    mcp_config: BTreeMap<String, crate::config::McpServerConfig>,
    plugin_manifests: BTreeMap<String, PluginManifest>,
    plugin_packages: BTreeMap<String, PluginPackage>,
}

impl ToolCatalog {
    pub fn new(
        mcp_manager: Arc<crate::mcp::McpManager>,
        config: &crate::config::AppConfig,
    ) -> Self {
        Self {
            native_tools: vec![
                Arc::new(CalculatorTool),
                Arc::new(SystemTimeTool),
                Arc::new(FileReaderTool),
                Arc::new(FileWriterTool),
                Arc::new(ListFilesTool),
                Arc::new(MkdirTool),
                Arc::new(EditFileTool),
            ],
            mcp_manager,
            mcp_runtime: config.mcp_runtime.clone(),
            mcp_config: config.mcp_servers.clone(),
            plugin_manifests: BTreeMap::new(),
            plugin_packages: BTreeMap::new(),
        }
    }

    /// Build a catalog and attach plugins discovered from
    /// `$AGENTJAX_HOME/plugins`. Discovery errors are logged and leave the
    /// native/MCP catalog intact so a broken local plugin cannot prevent chat.
    pub fn new_with_home_plugins(
        mcp_manager: Arc<crate::mcp::McpManager>,
        config: &crate::config::AppConfig,
    ) -> Self {
        let fallback_mcp_manager = mcp_manager.clone();
        let catalog = Self::new(mcp_manager, config);
        match discover_home_plugin_packages() {
            Ok(packages) => match catalog.with_plugin_packages(packages) {
                Ok(catalog) => catalog,
                Err(err) => {
                    log::warn!("Failed to register plugins from AGENTJAX_HOME/plugins: {err}");
                    Self::new(fallback_mcp_manager, config)
                }
            },
            Err(err) => {
                log::warn!("Failed to discover plugins from AGENTJAX_HOME/plugins: {err}");
                catalog
            }
        }
    }

    /// Attach validated plugin manifests to the catalog.
    ///
    /// This keeps plugin discovery separate from execution: manifests can be
    /// normalized into provider schemas now, while the deno_core execution path
    /// can be wired in behind the same catalog entry later.
    pub fn with_plugin_manifests(
        mut self,
        manifests: impl IntoIterator<Item = PluginManifest>,
    ) -> Result<Self, String> {
        for manifest in manifests {
            manifest.validate()?;
            if self.plugin_manifests.contains_key(&manifest.id) {
                return Err(format!("plugin '{}' is already registered", manifest.id));
            }
            self.plugin_manifests.insert(manifest.id.clone(), manifest);
        }
        Ok(self)
    }

    /// Attach validated plugin packages to the catalog and enable execution
    /// through the deno_core runtime bridge.
    pub fn with_plugin_packages(
        mut self,
        packages: impl IntoIterator<Item = PluginPackage>,
    ) -> Result<Self, String> {
        let mut runtime = DenoCorePluginRuntime::new(
            deno_core::RuntimeOptions::default(),
            SandboxPolicy::default(),
        );
        let mut manifests = BTreeMap::new();
        let mut registered_packages = BTreeMap::new();
        for package in packages {
            let manifest = package.manifest.clone();
            if manifests.contains_key(&manifest.id)
                || self.plugin_manifests.contains_key(&manifest.id)
            {
                return Err(format!("plugin '{}' is already registered", manifest.id));
            }
            runtime
                .register_package(package.clone())
                .map_err(|err| err.to_string())?;
            manifests.insert(manifest.id.clone(), manifest);
            registered_packages.insert(package.manifest.id.clone(), package);
        }
        self.plugin_manifests.extend(manifests);
        self.plugin_packages.extend(registered_packages);
        Ok(self)
    }

    pub async fn snapshot(&self, context: &ToolExecutionContext) -> ToolCatalogSnapshot {
        let mounted_servers = self.load_persisted_mounted_servers(context);
        self.snapshot_with_format_and_mounted_servers(
            ToolSchemaFormat::Responses,
            context,
            &mounted_servers,
        )
        .await
    }

    pub async fn snapshot_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> ToolCatalogSnapshot {
        let mounted_servers = self.load_persisted_mounted_servers(context);
        self.snapshot_with_format_and_mounted_servers(format, context, &mounted_servers)
            .await
    }

    pub(crate) async fn snapshot_with_format_and_mounted_servers(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
        mounted_servers: &MountedToolSourceSessions,
    ) -> ToolCatalogSnapshot {
        let mut schemas = Vec::new();
        let mut active_tool_names = HashSet::new();
        let mut entries = HashMap::new();
        let mut presentations = HashMap::new();

        for tool in &self.native_tools {
            let schema = tool.to_schema_with_format(format);
            let tool_name = tool.name().to_string();
            presentations.insert(tool_name.clone(), tool.presentation());
            insert_snapshot_tool(
                &mut schemas,
                schema,
                &mut active_tool_names,
                &mut entries,
                tool_name,
                ToolSnapshotEntry::Native(tool.clone()),
            );
        }

        for (server_id, server_config) in &self.mcp_config {
            if !server_config.enabled {
                continue;
            }

            let resolved_server_config =
                self.resolve_server_config_with_workspace_fallback(server_config, context);

            if server_config.unfolded {
                match self
                    .mcp_manager
                    .list_tools(server_id, &resolved_server_config, &self.mcp_runtime)
                    .await
                {
                    Ok(raw_tools) => {
                        let mounted_tools = normalize_mcp_tool_definitions(raw_tools);
                        for tool in mounted_tools {
                            let prefixed_name = prefixed_mcp_tool_name(server_id, &tool.tool_name);
                            presentations.insert(
                                prefixed_name.clone(),
                                ToolPresentation {
                                    display_name: tool.display_name.clone(),
                                    description: tool.description.clone(),
                                    icon: tool.icon.clone(),
                                },
                            );
                            insert_snapshot_tool(
                                &mut schemas,
                                format_tool_schema(
                                    format,
                                    &prefixed_name,
                                    &tool.description,
                                    tool.input_schema.clone(),
                                ),
                                &mut active_tool_names,
                                &mut entries,
                                prefixed_name,
                                ToolSnapshotEntry::Mcp {
                                    server_id: server_id.clone(),
                                    tool_name: tool.tool_name.clone(),
                                    server_config: resolved_server_config.clone(),
                                },
                            );
                        }
                    }
                    Err(err) => {
                        log::error!(
                            "Failed to list tools for unfolded MCP server '{}': {}",
                            server_id,
                            err
                        );
                    }
                }
            } else {
                let mounted_session = mounted_servers.get(server_id).cloned();
                let control_tool_name = mount_tool_name_for_server(server_id);
                presentations.insert(
                    control_tool_name.clone(),
                    presentation_for_manage_mcp_server(server_id),
                );
                insert_snapshot_tool(
                    &mut schemas,
                    build_manage_mcp_server_tool_schema(
                        format,
                        server_id,
                        mounted_session.is_some(),
                    ),
                    &mut active_tool_names,
                    &mut entries,
                    control_tool_name,
                    ToolSnapshotEntry::ManageMcpServer {
                        server_id: server_id.clone(),
                        server_config: resolved_server_config.clone(),
                        mounted_session: mounted_session.clone(),
                    },
                );

                if let Some(mounted) = mounted_servers.get(server_id) {
                    for tool in &mounted.tools {
                        let prefixed_name = prefixed_mcp_tool_name(server_id, &tool.tool_name);
                        presentations.insert(
                            prefixed_name.clone(),
                            ToolPresentation {
                                display_name: tool.display_name.clone(),
                                description: tool.description.clone(),
                                icon: tool.icon.clone(),
                            },
                        );
                        insert_snapshot_tool(
                            &mut schemas,
                            format_tool_schema(
                                format,
                                &prefixed_name,
                                &tool.description,
                                tool.input_schema.clone(),
                            ),
                            &mut active_tool_names,
                            &mut entries,
                            prefixed_name,
                            ToolSnapshotEntry::Mcp {
                                server_id: mounted.source_id.clone(),
                                tool_name: tool.tool_name.clone(),
                                server_config: mounted
                                    .mcp_config
                                    .as_ref()
                                    .cloned()
                                    .unwrap_or_else(|| resolved_server_config.clone()),
                            },
                        );
                    }
                }
            }
        }

        for registered_tool in self
            .plugin_manifests
            .values()
            .flat_map(registered_tools_for_manifest)
        {
            let prefixed_name =
                prefixed_plugin_tool_name(&registered_tool.plugin_id, &registered_tool.tool.name);
            let display_name = if registered_tool.tool.display_name.trim().is_empty() {
                humanize_tool_name(&registered_tool.tool.name)
            } else {
                registered_tool.tool.display_name.clone()
            };
            presentations.insert(
                prefixed_name.clone(),
                ToolPresentation {
                    display_name,
                    description: registered_tool.tool.description.clone(),
                    icon: registered_tool.tool.icon.clone(),
                },
            );
            insert_snapshot_tool(
                &mut schemas,
                format_tool_schema(
                    format,
                    &prefixed_name,
                    &registered_tool.tool.description,
                    registered_tool.tool.input_schema.clone(),
                ),
                &mut active_tool_names,
                &mut entries,
                prefixed_name,
                ToolSnapshotEntry::Plugin {
                    plugin_id: registered_tool.plugin_id.clone(),
                    tool_name: registered_tool.tool.name,
                    package: self
                        .plugin_packages
                        .get(&registered_tool.plugin_id)
                        .cloned(),
                },
            );
        }

        if let Err(err) = self.apply_conversation_dynamic_tools(
            format,
            context,
            &mut schemas,
            &mut active_tool_names,
            &mut entries,
            &mut presentations,
        ) {
            log::warn!(
                "Failed to apply conversation dynamic tools for {:?}: {}",
                context.conversation_id,
                err
            );
        }

        for (tool_name, schema, entry, presentation) in [
            (
                START_BACKGROUND_TOOL_NAME.to_string(),
                build_start_background_tool_schema(format),
                ToolSnapshotEntry::StartBackgroundTool,
                ToolPresentation::new(
                    "Start Background Tool",
                    "Starts a tool as a background job.",
                    Some("Rocket"),
                ),
            ),
            (
                WAIT_BACKGROUND_TOOL_NAME.to_string(),
                build_wait_background_tool_schema(format),
                ToolSnapshotEntry::WaitBackgroundTool,
                ToolPresentation::new(
                    "Wait Background Tool",
                    "Waits for a background tool job.",
                    Some("Timer"),
                ),
            ),
            (
                CANCEL_BACKGROUND_TOOL_NAME.to_string(),
                build_cancel_background_tool_schema(format),
                ToolSnapshotEntry::CancelBackgroundTool,
                ToolPresentation::new(
                    "Cancel Background Tool",
                    "Cancels a background tool job.",
                    Some("CircleStop"),
                ),
            ),
            (
                LIST_BACKGROUND_TOOLS_NAME.to_string(),
                build_list_background_tools_schema(format),
                ToolSnapshotEntry::ListBackgroundTools,
                ToolPresentation::new(
                    "List Background Tools",
                    "Lists background tool jobs.",
                    Some("ListChecks"),
                ),
            ),
        ] {
            presentations.insert(tool_name.clone(), presentation);
            insert_snapshot_tool(
                &mut schemas,
                schema,
                &mut active_tool_names,
                &mut entries,
                tool_name,
                entry,
            );
        }

        ToolCatalogSnapshot {
            schemas,
            active_tool_names,
            entries,
            presentations,
            mcp_manager: self.mcp_manager.clone(),
            mcp_runtime: self.mcp_runtime.clone(),
        }
    }

    pub async fn list_schemas(&self, context: &ToolExecutionContext) -> Vec<Value> {
        self.snapshot(context).await.schemas
    }

    pub async fn list_schemas_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> Vec<Value> {
        self.snapshot_with_format(format, context).await.schemas
    }

    pub async fn execute(
        &self,
        prefixed_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> Result<Value, String> {
        self.snapshot(context)
            .await
            .execute(prefixed_name, arguments, context)
            .await
    }
}
