mod background;
mod dynamic;
mod entry;
mod manager_snapshot;
mod mounted;
mod names;
mod plugin_execution;
mod plugin_manager_snapshot;
mod schemas;
mod snapshot;
mod types;



use crate::agentjax_err;
use crate::plugin_runtime::{
    PluginManifest, PluginPackage, discover_all_plugin_packages, prefixed_plugin_tool_name,
    registered_tools_for_manifest,
};
use crate::tools::knowledge_base_tools::{
    KbGetTool, KbIndexTool, KbListTool, KbSearchTool,
};
use crate::tools::memory_tools::{MemoryRecallTool, MemorySearchTool, MemoryWriteTool};
use crate::tools::sub_agent_tools::SubAgentTool;
use crate::tools::{
    CalculatorTool, EditFileTool, FileReaderTool, FileWriterTool, ListFilesTool, MkdirTool,
    SystemTimeTool, Tool, ToolExecutionContext, ToolPresentation, ToolSchemaFormat,
    format_tool_schema, humanize_tool_name,
};
pub(crate) use entry::{
    ContextGating, McpCollectedTool, PluginCollectedTool, RegisteredToolInfo, ToolCategory,
    ToolEntry,
};
#[allow(unused_imports)]
pub use manager_snapshot::{
    ToolManagerSchemaFormat, ToolManagerSnapshot, ToolManagerSnapshotRequest,
    ToolManagerSourceSnapshot, ToolManagerSourceType, ToolManagerToolSnapshot,
};
use names::{
    mount_tool_name_for_server, prefixed_mcp_tool_name, presentation_for_manage_mcp_server,
};
#[allow(unused_imports)]
pub use plugin_manager_snapshot::{
    DeclaredPermissions, EffectivePermissions, PluginEntryPolicyPaths, PluginEntrySnapshot,
    PluginManagerSnapshot, build_plugin_manager_snapshot,
};
use schemas::{
    BACKGROUND_TASK_NAME, build_background_task_schema, build_manage_mcp_server_tool_schema,
    normalize_mcp_tool_definitions,
};
use serde_json::Value;
pub use snapshot::ToolCatalogSnapshot;
use snapshot::{ToolSnapshotEntry, insert_snapshot_tool};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
#[allow(unused_imports)]
pub use types::{
    MountedToolDefinition, MountedToolSourceSession, MountedToolSourceSessions,
    ToolCatalogStateChange,
};

pub struct ToolCatalog {
    /// Unified tool registry — replaces the old split between `native_tools`
    /// and `context_tools`.  Each entry carries category and gating metadata
    /// so both model snapshots and tool manager snapshots iterate the same
    /// source of truth.
    tools: Vec<ToolEntry>,
    mcp_manager: Arc<crate::mcp::McpManager>,
    mcp_runtime: crate::config::McpRuntimeConfig,
    mcp_config: BTreeMap<String, crate::config::McpServerConfig>,
    tool_manager: crate::config::ToolManagerConfig,
    plugin_manager: crate::config::PluginManagerConfig,
    plugin_manifests: BTreeMap<String, PluginManifest>,
    plugin_packages: BTreeMap<String, PluginPackage>,
}

impl ToolCatalog {
    pub fn new(
        mcp_manager: Arc<crate::mcp::McpManager>,
        config: &crate::config::AppConfig,
        agent_config: &crate::config::AgentConfig,
    ) -> Self {
        use crate::lcm::{LcmDescribeTool, LcmExpandTool, LcmGrepTool, LlmMapTool};

        Self {
            tools: vec![
                // ── Native tools ────────────────────────────────────────
                ToolEntry::native(Arc::new(CalculatorTool)),
                ToolEntry::native(Arc::new(SystemTimeTool)),
                ToolEntry::native(Arc::new(FileReaderTool)),
                ToolEntry::native(Arc::new(FileWriterTool)),
                ToolEntry::native(Arc::new(ListFilesTool)),
                ToolEntry::native(Arc::new(MkdirTool)),
                ToolEntry::native(Arc::new(EditFileTool)),
                ToolEntry::native(Arc::new(SubAgentTool)),
                // ── Context tools: LCM ──────────────────────────────────
                // Registered without a store so they appear in tool lists
                // from the start.  When a real LcmEngine is available,
                // update_lcm_context_tools() wires in the real store-backed
                // instances so execute() can operate.
                ToolEntry::context_always(Arc::new(LlmMapTool)),
                ToolEntry::context_always(Arc::new(LcmGrepTool::without_store())),
                ToolEntry::context_always(Arc::new(LcmDescribeTool::without_store())),
                ToolEntry::context_always(Arc::new(LcmExpandTool::without_store())),
                // ── Context tools: Memory (gated) ───────────────────────
                ToolEntry::context(
                    Arc::new(MemoryWriteTool),
                    ContextGating::MemorySubAgentOnly,
                ),
                ToolEntry::context(
                    Arc::new(MemorySearchTool),
                    ContextGating::MemoryEnabled,
                ),
                ToolEntry::context(
                    Arc::new(MemoryRecallTool),
                    ContextGating::MemoryEnabled,
                ),
                // ── Context tools: Knowledge base ───────────────────────
                ToolEntry::context_always(Arc::new(KbListTool)),
                ToolEntry::context_always(Arc::new(KbSearchTool)),
                ToolEntry::context_always(Arc::new(KbGetTool)),
                ToolEntry::context_always(Arc::new(KbIndexTool)),
            ],
            mcp_manager,
            mcp_runtime: config.mcp.runtime(),
            mcp_config: config.mcp.servers.clone(),
            tool_manager: agent_config.tool_manager.clone(),
            plugin_manager: config.plugin_manager.clone(),
            plugin_manifests: BTreeMap::new(),
            plugin_packages: BTreeMap::new(),
        }
    }

    /// Build a catalog and attach built-in plugins plus plugins discovered from
    /// `$AGENTJAX_HOME/plugins`. Discovery errors are logged and leave the
    /// native/MCP catalog intact so a broken local plugin cannot prevent chat.
    pub fn new_with_home_plugins(
        mcp_manager: Arc<crate::mcp::McpManager>,
        config: &crate::config::AppConfig,
        agent_config: &crate::config::AgentConfig,
    ) -> Self {
        let fallback_mcp_manager = mcp_manager.clone();
        let catalog = Self::new(mcp_manager, config, agent_config);
        match discover_all_plugin_packages() {
            Ok(packages) => match catalog.with_plugin_packages(packages) {
                Ok(catalog) => catalog,
                Err(err) => {
                    log::warn!("Failed to register plugins from AGENTJAX_HOME/plugins: {err}");
                    Self::new(fallback_mcp_manager, config, agent_config)
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
    #[allow(dead_code)] // Reserved for future use
    pub fn with_plugin_manifests(
        mut self,
        manifests: impl IntoIterator<Item = PluginManifest>,
    ) -> crate::error::AgentJaxResult<Self> {
        for manifest in manifests {
            manifest.validate().map_err(|e| e.to_string())?;
            if self.plugin_manifests.contains_key(&manifest.id) {
                return Err(agentjax_err!(
                    format!("plugin '{}' is already registered", manifest.id),
                    Config
                ));
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
    ) -> crate::error::AgentJaxResult<Self> {
        let mut manifests = BTreeMap::new();
        let mut registered_packages = BTreeMap::new();
        for package in packages {
            let manifest = package.manifest.clone();
            if manifests.contains_key(&manifest.id)
                || self.plugin_manifests.contains_key(&manifest.id)
            {
                return Err(agentjax_err!(
                    format!("plugin '{}' is already registered", manifest.id),
                    Config
                ));
            }
            // Validate the manifest without creating a JsRuntime -
            // tool execution creates a temp PluginInstance on demand.
            manifest.validate().map_err(|err| {
                format!("plugin '{}' has an invalid manifest: {err}", manifest.id)
            })?;
            manifests.insert(manifest.id.clone(), manifest);
            registered_packages.insert(package.manifest.id.clone(), package);
        }
        self.plugin_manifests.extend(manifests);
        self.plugin_packages.extend(registered_packages);
        Ok(self)
    }

    #[allow(dead_code)] // Reserved for future use
    pub async fn snapshot(&self, context: &ToolExecutionContext) -> ToolCatalogSnapshot {
        let mounted_servers = self.load_persisted_mounted_servers(context);
        self.snapshot_with_format_and_mounted_servers(
            ToolSchemaFormat::Responses,
            context,
            &mounted_servers,
        )
        .await
    }

    #[allow(dead_code)] // Reserved for future use
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

        // ── Unified registered-tool enumeration ─────────────────────────
        for info in self.collect_registered_tools(context) {
            if !info.enabled {
                continue;
            }
            let schema = format_tool_schema(
                format,
                &info.name,
                &info.description,
                info.schema.clone(),
            );
            presentations.insert(
                info.name.clone(),
                ToolPresentation::new(
                    info.display_name,
                    info.description,
                    info.icon.clone(),
                ),
            );
            insert_snapshot_tool(
                &mut schemas,
                schema,
                &mut active_tool_names,
                &mut entries,
                info.name.clone(),
                ToolSnapshotEntry::Native(info.tool.clone()),
            );
        }

        for (server_id, server_config) in &self.mcp_config {
            if !server_config.enabled || !self.mcp_source_enabled(server_id) {
                continue;
            }

            let resolved_server_config =
                self.resolve_server_config_with_workspace_fallback(server_config, context);

            if self.mcp_source_unfolded(server_id, server_config) {
                match self
                    .mcp_manager
                    .list_tools(server_id, &resolved_server_config, &self.mcp_runtime)
                    .await
                {
                    Ok(raw_tools) => {
                        let mounted_tools = normalize_mcp_tool_definitions(raw_tools);
                        for tool in mounted_tools {
                            if !self.mcp_tool_enabled(server_id, &tool.tool_name) {
                                continue;
                            }
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
                        let collected = self.collect_mcp_tool(server_id, tool);
                        if !collected.enabled {
                            continue;
                        }
                        let prefixed_name =
                            prefixed_mcp_tool_name(server_id, &collected.tool_name);
                        presentations.insert(
                            prefixed_name.clone(),
                            ToolPresentation {
                                display_name: collected.display_name.clone(),
                                description: collected.description.clone(),
                                icon: collected.icon.clone(),
                            },
                        );
                        insert_snapshot_tool(
                            &mut schemas,
                            format_tool_schema(
                                format,
                                &prefixed_name,
                                &collected.description,
                                collected.input_schema,
                            ),
                            &mut active_tool_names,
                            &mut entries,
                            prefixed_name,
                            ToolSnapshotEntry::Mcp {
                                server_id: mounted.source_id.clone(),
                                tool_name: collected.tool_name,
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

        for info in self.collect_plugin_tools() {
            if !info.enabled {
                continue;
            }
            let prefixed_name = prefixed_plugin_tool_name(&info.plugin_id, &info.tool_name);
            let plugin_id = info.plugin_id.clone();
            presentations.insert(
                prefixed_name.clone(),
                ToolPresentation {
                    display_name: info.display_name,
                    description: info.description.clone(),
                    icon: info.icon.clone(),
                },
            );
            insert_snapshot_tool(
                &mut schemas,
                format_tool_schema(format, &prefixed_name, &info.description, info.input_schema),
                &mut active_tool_names,
                &mut entries,
                prefixed_name,
                ToolSnapshotEntry::Plugin {
                    plugin_id: info.plugin_id,
                    tool_name: info.tool_name,
                    package: self.plugin_packages.get(&plugin_id).cloned(),
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

        if self.native_tool_enabled(BACKGROUND_TASK_NAME) {
            let bg_name = BACKGROUND_TASK_NAME.to_string();
            presentations.insert(
                bg_name.clone(),
                ToolPresentation::new(
                    "Background Task",
                    "Manages background tool jobs — start, wait, cancel, or list.",
                    Some("Rocket"),
                ),
            );
            insert_snapshot_tool(
                &mut schemas,
                build_background_task_schema(format),
                &mut active_tool_names,
                &mut entries,
                bg_name,
                ToolSnapshotEntry::BackgroundTask,
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

    #[allow(dead_code)] // Reserved for future use
    pub async fn list_schemas(&self, context: &ToolExecutionContext) -> Vec<Value> {
        self.snapshot(context).await.schemas
    }

    #[allow(dead_code)] // Reserved for future use
    pub async fn list_schemas_with_format(
        &self,
        format: ToolSchemaFormat,
        context: &ToolExecutionContext,
    ) -> Vec<Value> {
        self.snapshot_with_format(format, context).await.schemas
    }

    #[allow(dead_code)] // Reserved for future use
    pub async fn execute(
        &self,
        prefixed_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> crate::error::AgentJaxResult<Value> {
        self.snapshot(context)
            .await
            .execute(prefixed_name, arguments, context)
            .await
    }

    // ── Shared tool enumeration ─────────────────────────────────────────
    //
    // Called by both the model snapshot and the tool manager snapshot so
    // that native + context tools go through identical filtering and gating.

    /// Enumerate all registered (native + context) tools with gating applied.
    ///
    /// Returns a `Vec<RegisteredToolInfo>` where each entry's `enabled` field
    /// reflects whether the tool should be visible given the current context
    /// (policy + memory gating).  Both snapshot paths consume this same data
    /// to avoid duplicated filtering logic.
    pub(crate) fn collect_registered_tools(
        &self,
        context: &ToolExecutionContext,
    ) -> Vec<RegisteredToolInfo> {
        let memory_enabled = context
            .agent_config
            .as_ref()
            .map(|a| a.memory.enabled)
            .or_else(|| {
                crate::config::load_agent_config(crate::config::constants::DEFAULT_AGENT_ID)
                    .ok()
                    .map(|a| a.normalize().memory.enabled)
            })
            .unwrap_or(false);

        self.tools
            .iter()
            .map(|entry| {
                let enabled = self.entry_enabled(entry)
                    && self.entry_available_in_context(entry, memory_enabled, context);
                RegisteredToolInfo::from_entry(entry, enabled)
            })
            .collect()
    }

    /// Enumerate all plugin tools with source-level filtering applied.
    ///
    /// Returns a `Vec<PluginCollectedTool>` where each entry's `enabled` field
    /// reflects `plugin_tool_enabled()`.  Both snapshot paths consume the same
    /// data to avoid duplicated filtering logic.
    pub(crate) fn collect_plugin_tools(&self) -> Vec<PluginCollectedTool> {
        self.plugin_manifests
            .values()
            .flat_map(registered_tools_for_manifest)
            .map(|registered| {
                let enabled =
                    self.plugin_tool_enabled(&registered.plugin_id, &registered.tool.name);
                let display_name = if registered.tool.display_name.trim().is_empty() {
                    humanize_tool_name(&registered.tool.name)
                } else {
                    registered.tool.display_name.clone()
                };
                PluginCollectedTool {
                    plugin_id: registered.plugin_id,
                    tool_name: registered.tool.name,
                    display_name,
                    description: registered.tool.description,
                    icon: registered.tool.icon,
                    input_schema: registered.tool.input_schema,
                    enabled,
                }
            })
            .collect()
    }

    // ── Unified enablement & gating ──────────────────────────────────────
    //
    // These replace the old split between native_tool_enabled() and
    // context_tool_enabled().  Category-specific logic is now driven by
    // the ToolEntry metadata rather than separate Vecs.

    /// Check whether a tool entry's base enablement policy allows it.
    ///
    /// Native tools are gated by `tool_manager.native_tools.*.enabled`.
    /// Context tools are forced enabled (the agent depends on them).
    pub(crate) fn entry_enabled(&self, entry: &ToolEntry) -> bool {
        match entry.category {
            ToolCategory::Native => self.native_tool_enabled(entry.name()),
            ToolCategory::Context => true,
        }
    }

    /// Check whether a context tool is gatable by the current runtime state.
    ///
    /// Native tools always pass. Context tools may be hidden when the memory
    /// system is disabled or when running outside the Memory sub-agent.
    pub(crate) fn entry_available_in_context(
        &self,
        entry: &ToolEntry,
        memory_enabled: bool,
        context: &ToolExecutionContext,
    ) -> bool {
        match entry.context_gating {
            ContextGating::None => true,
            ContextGating::MemoryEnabled => memory_enabled,
            ContextGating::MemorySubAgentOnly => context.is_memory_sub_agent,
        }
    }

    /// Whether a native tool is enabled in the tool manager policy.
    ///
    /// Kept as a convenience for the tool manager snapshot and background
    /// task checks; prefer `entry_enabled()` for unified dispatch.
    /// Convert a `MountedToolDefinition` + server metadata into an
    /// `McpCollectedTool`, applying the per-tool enablement policy.
    ///
    /// Both snapshot paths use this to avoid duplicating the enablement
    /// check and field mapping.
    pub(crate) fn collect_mcp_tool(
        &self,
        server_id: &str,
        tool: &MountedToolDefinition,
    ) -> McpCollectedTool {
        McpCollectedTool {
            tool_name: tool.tool_name.clone(),
            display_name: tool.display_name.clone(),
            description: tool.description.clone(),
            icon: tool.icon.clone(),
            input_schema: tool.input_schema.clone(),
            enabled: self.mcp_tool_enabled(server_id, &tool.tool_name),
        }
    }

    pub(crate) fn native_tool_enabled(&self, tool_name: &str) -> bool {
        self.tool_manager
            .native_tools
            .get(&tool_name.to_ascii_lowercase())
            .map(|policy| policy.enabled)
            .unwrap_or(true)
    }

    /// Wire LCM store-backed tools into the unified registry.
    ///
    /// Call this after construction (once an `LcmEngine` is available) to
    /// replace the placeholder `without_store()` instances with real
    /// store-backed ones so `execute()` can operate.
    pub fn update_lcm_context_tools(&mut self, lcm_store: Arc<crate::lcm::LcmStore>) {
        use crate::lcm::{LcmDescribeTool, LcmExpandTool, LcmGrepTool};

        let replacements: [(&str, Arc<dyn Tool>); 3] = [
            ("lcm_grep", Arc::new(LcmGrepTool::new(lcm_store.clone()))),
            ("lcm_describe", Arc::new(LcmDescribeTool::new(lcm_store.clone()))),
            ("lcm_expand", Arc::new(LcmExpandTool::new(lcm_store))),
        ];
        for (name, new_tool) in &replacements {
            if let Some(entry) = self.tools.iter_mut().find(|e| e.tool.name() == *name) {
                entry.tool = new_tool.clone();
            }
        }
    }

    /// Check if the plugin is enabled at the plugin-manager level.
    pub(crate) fn plugin_enabled(&self, plugin_id: &str) -> bool {
        self.plugin_manager
            .plugins
            .get(&plugin_id.to_ascii_lowercase())
            .map(|entry| entry.enabled)
            .unwrap_or(true)
    }

    pub(crate) fn plugin_source_enabled(&self, plugin_id: &str) -> bool {
        if !self.plugin_enabled(plugin_id) {
            return false;
        }
        self.tool_manager
            .plugin_tools
            .get(&plugin_id.to_ascii_lowercase())
            .map(|policy| policy.enabled)
            .unwrap_or(true)
    }

    pub(crate) fn plugin_tool_enabled(&self, plugin_id: &str, tool_name: &str) -> bool {
        if !self.plugin_enabled(plugin_id) {
            return false;
        }
        let plugin_id = plugin_id.to_ascii_lowercase();
        let tool_name = tool_name.to_ascii_lowercase();
        self.tool_manager
            .plugin_tools
            .get(&plugin_id)
            .map(|policy| {
                policy.enabled
                    && policy
                        .tools
                        .get(&tool_name)
                        .map(|tool_policy| tool_policy.enabled)
                        .unwrap_or(true)
            })
            .unwrap_or(true)
    }

    pub(crate) fn mcp_source_enabled(&self, server_id: &str) -> bool {
        self.tool_manager
            .mcp_tools
            .get(&server_id.to_ascii_lowercase())
            .map(|policy| policy.enabled)
            .unwrap_or(true)
    }

    pub(crate) fn mcp_tool_enabled(&self, server_id: &str, tool_name: &str) -> bool {
        let server_id = server_id.to_ascii_lowercase();
        let tool_name = tool_name.to_ascii_lowercase();
        self.tool_manager
            .mcp_tools
            .get(&server_id)
            .map(|policy| {
                policy.enabled
                    && policy
                        .tools
                        .get(&tool_name)
                        .map(|tool_policy| tool_policy.enabled)
                        .unwrap_or(true)
            })
            .unwrap_or(true)
    }

    pub(crate) fn mcp_source_unfolded(
        &self,
        server_id: &str,
        server_config: &crate::config::McpServerConfig,
    ) -> bool {
        self.tool_manager
            .mcp_tools
            .get(&server_id.to_ascii_lowercase())
            .and_then(|policy| policy.exposure.as_deref())
            .map(|exposure| exposure == "unfolded")
            .unwrap_or(server_config.unfolded)
    }
}
