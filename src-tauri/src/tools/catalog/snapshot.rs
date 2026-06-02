use super::background::{
    background_job_id, background_tool_arguments, background_tool_name, background_wait_timeout_ms,
    execute_backgroundable_entry, is_backgroundable_entry,
};
use super::names::{mount_tool_name_for_server, prefixed_mcp_tool_name};
use super::plugin_execution::execute_plugin_package_tool;
use super::types::{MountedToolSourceSession, ToolCatalogExecution, ToolCatalogStateChange};
use crate::config::AppConfig;
use crate::plugin_runtime::PluginPackage;
use crate::tools::{Tool, ToolExecutionContext, ToolPresentation, background_jobs};
use futures_util::FutureExt;
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};
use std::panic::AssertUnwindSafe;
use std::sync::Arc;

#[derive(Clone)]
pub(super) enum ToolSnapshotEntry {
    Native(Arc<dyn Tool>),
    Mcp {
        server_id: String,
        tool_name: String,
        server_config: crate::config::McpServerConfig,
    },
    Plugin {
        plugin_id: String,
        tool_name: String,
        package: Option<PluginPackage>,
    },
    ManageMcpServer {
        server_id: String,
        server_config: crate::config::McpServerConfig,
        mounted_session: Option<MountedToolSourceSession>,
    },
    /// Consolidated background task tool — replaces the old four-variant
    /// design (StartBackgroundTool / WaitBackgroundTool / CancelBackgroundTool
    /// / ListBackgroundTools). Uses an `action` field to dispatch.
    BackgroundTask,
}

/// Insert or replace a tool in every snapshot index used for schema emission and dispatch.
pub(super) fn insert_snapshot_tool(
    schemas: &mut Vec<Value>,
    schema: Value,
    active_tool_names: &mut HashSet<String>,
    entries: &mut HashMap<String, ToolSnapshotEntry>,
    tool_name: String,
    entry: ToolSnapshotEntry,
) {
    active_tool_names.insert(tool_name.clone());
    entries.insert(tool_name.clone(), entry);

    if let Some(existing_idx) = schemas.iter().position(|existing| {
        existing.get("name").and_then(Value::as_str) == Some(tool_name.as_str())
            || existing
                .get("function")
                .and_then(|function| function.get("name"))
                .and_then(Value::as_str)
                == Some(tool_name.as_str())
    }) {
        schemas[existing_idx] = schema;
    } else {
        schemas.push(schema);
    }
}

/// Turn-scoped tool snapshot.
///
/// The model-visible tool list and local execution dispatch both read from the
/// same frozen snapshot so a turn cannot drift if MCP tools are reconfigured or
/// refreshed midway through a tool loop.
#[derive(Clone)]
pub struct ToolCatalogSnapshot {
    pub(super) schemas: Vec<Value>,
    pub(super) active_tool_names: HashSet<String>,
    pub(super) entries: HashMap<String, ToolSnapshotEntry>,
    pub(super) presentations: HashMap<String, ToolPresentation>,
    pub(super) mcp_manager: Arc<crate::mcp::McpManager>,
    pub(super) mcp_runtime: crate::config::McpRuntimeConfig,
    /// Application config carried through from the tool execution context.
    /// Available to tools that need to read global configuration at runtime.
    pub(super) app_config: Option<Arc<AppConfig>>,
}

impl ToolCatalogSnapshot {
    pub fn schemas(&self) -> &[Value] {
        &self.schemas
    }

    pub fn active_tool_names(&self) -> &HashSet<String> {
        &self.active_tool_names
    }

    pub fn presentation_for(&self, tool_name: &str) -> Option<&ToolPresentation> {
        self.presentations.get(tool_name)
    }

    pub async fn execute(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> crate::error::AgentJaxResult<Value> {
        Ok(self
            .execute_with_effects(tool_name, arguments, context)
            .await?
            .output)
    }

    pub(crate) async fn execute_with_effects(
        &self,
        tool_name: &str,
        arguments: &Value,
        context: &ToolExecutionContext,
    ) -> crate::error::AgentJaxResult<ToolCatalogExecution> {
        let entry = self
            .entries
            .get(tool_name)
            .ok_or_else(|| format!("Tool '{}' not found in turn snapshot", tool_name))?;

        match entry {
            ToolSnapshotEntry::Native(tool) => Ok(ToolCatalogExecution {
                output: tool.execute(arguments, context).await?,
                state_changes: Vec::new(),
            }),
            ToolSnapshotEntry::Mcp {
                server_id,
                tool_name,
                server_config,
            } => Ok(ToolCatalogExecution {
                output: self
                    .mcp_manager
                    .call_tool(
                        server_id,
                        server_config,
                        &self.mcp_runtime,
                        tool_name,
                        arguments.clone(),
                    )
                    .await?,
                state_changes: Vec::new(),
            }),
            ToolSnapshotEntry::Plugin {
                plugin_id,
                tool_name,
                package,
            } => {
                let package = package.as_ref().ok_or_else(|| {
                    format!(
                        "Plugin tool '{}::{}' is declared but no executable plugin package is attached",
                        plugin_id, tool_name
                    )
                })?;
                execute_plugin_package_tool(package, plugin_id, tool_name, arguments, context)
                    .map_err(Into::into)
            }
            ToolSnapshotEntry::BackgroundTask => {
                use super::schemas::BACKGROUND_TASK_NAME;

                let action = arguments
                    .get("action")
                    .and_then(Value::as_str)
                    .unwrap_or("");
                match action {
                    "start" => {
                        let target_tool_name = background_tool_name(arguments)?;
                        let target_arguments = background_tool_arguments(arguments);
                        let target_entry = self
                            .entries
                            .get(&target_tool_name)
                            .cloned()
                            .ok_or_else(|| {
                                format!(
                                    "Cannot start background tool job because target tool '{}' is not active in this turn",
                                    target_tool_name
                                )
                            })?;
                        if !is_backgroundable_entry(&target_entry) {
                            return Err(crate::error::AgentJaxError::tool(format!(
                                "Tool '{}' cannot run in the background. Only native and MCP tools are supported.",
                                target_tool_name
                            )));
                        }

                        let job = background_jobs::start_job_for_conversation(
                            target_tool_name.clone(),
                            context.conversation_id.clone(),
                        );
                        let job_id = background_jobs::job_id(&job);
                        let context = context.clone();
                        let mcp_manager = self.mcp_manager.clone();
                        let mcp_runtime = self.mcp_runtime.clone();
                        let job_for_task = job.clone();

                        let handle = tokio::spawn(async move {
                            // A background job must always become terminal. Convert a
                            // panicking target tool into a failed job instead of leaving
                            // waiters/listeners with a permanent in-progress snapshot.
                            let result = AssertUnwindSafe(execute_backgroundable_entry(
                                target_entry,
                                target_arguments,
                                context,
                                mcp_manager,
                                mcp_runtime,
                            ))
                            .catch_unwind()
                            .await
                            .unwrap_or_else(|_| Err::<Value, _>(crate::error::AgentJaxError::internal("Background tool task panicked")));
                            background_jobs::complete_job(&job_for_task, result);
                        });
                        background_jobs::register_job_handle(&job, handle);

                        Ok(ToolCatalogExecution {
                            output: json!({
                                "ok": true,
                                "role": "background_tool_starter",
                                "decision": "continue_or_await_later",
                                "jobId": job_id,
                                "toolName": target_tool_name,
                                "status": "in_progress",
                                "job": background_jobs::job_snapshot(&job),
                                "usage": {
                                    "wait": {
                                        "tool": BACKGROUND_TASK_NAME,
                                        "arguments": {
                                            "action": "wait",
                                            "jobId": job_id,
                                            "timeoutMs": background_jobs::DEFAULT_WAIT_TIMEOUT_MS
                                        }
                                    },
                                    "list": {
                                        "tool": BACKGROUND_TASK_NAME,
                                        "arguments": { "action": "list" }
                                    },
                                    "cancel": {
                                        "tool": BACKGROUND_TASK_NAME,
                                        "arguments": { "action": "cancel", "jobId": job_id }
                                    }
                                }
                            }),
                            state_changes: Vec::new(),
                        })
                    }
                    "wait" => {
                        let job_id = background_job_id(arguments)?;
                        let timeout_ms = background_wait_timeout_ms(arguments);
                        Ok(ToolCatalogExecution {
                            output: background_jobs::wait_for_job(
                                &job_id,
                                timeout_ms,
                                context.conversation_id.as_deref(),
                            )
                            .await?,
                            state_changes: Vec::new(),
                        })
                    }
                    "cancel" => {
                        let job_id = background_job_id(arguments)?;
                        Ok(ToolCatalogExecution {
                            output: background_jobs::cancel_job(
                                &job_id,
                                context.conversation_id.as_deref(),
                            )?,
                            state_changes: Vec::new(),
                        })
                    }
                    "list" => Ok(ToolCatalogExecution {
                        output: background_jobs::list_jobs(context.conversation_id.as_deref()),
                        state_changes: Vec::new(),
                    }),
                    _ => Err(crate::error::AgentJaxError::tool(format!(
                        "background_task: unknown action '{}'. Valid actions: start, wait, cancel, list",
                        action
                    ))),
                }
            },
            ToolSnapshotEntry::ManageMcpServer {
                server_id,
                server_config,
                mounted_session,
            } => {
                execute_manage_mcp_server(
                    &self.mcp_manager,
                    &self.mcp_runtime,
                    server_id,
                    server_config,
                    mounted_session,
                    arguments,
                )
                .await
                .map_err(Into::into)
            }
        }
    }
}

async fn execute_manage_mcp_server(
    mcp_manager: &crate::mcp::McpManager,
    mcp_runtime: &crate::config::McpRuntimeConfig,
    server_id: &str,
    server_config: &crate::config::McpServerConfig,
    mounted_session: &Option<MountedToolSourceSession>,
    arguments: &Value,
) -> Result<ToolCatalogExecution, String> {
    let requested_action = arguments
        .get("action")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| value.to_ascii_lowercase());
    let action = requested_action
        .as_deref()
        .unwrap_or(if mounted_session.is_some() {
            "status"
        } else {
            "mount"
        });
    let control_tool = mount_tool_name_for_server(server_id);

    match action {
        "status" => {
            let mounted_tools = mounted_session
                .as_ref()
                .map(|session| {
                    session
                        .tools
                        .iter()
                        .map(|tool| prefixed_mcp_tool_name(server_id, &tool.tool_name))
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            Ok(ToolCatalogExecution {
                output: json!({
                    "serverId": server_id,
                    "controlTool": control_tool,
                    "mounted": mounted_session.is_some(),
                    "mountedTools": mounted_tools,
                    "status": if mounted_session.is_some() { "mounted" } else { "unmounted" },
                }),
                state_changes: Vec::new(),
            })
        }
        "mount" => {
            if let Some(session) = mounted_session {
                let mounted_tools = session
                    .tools
                    .iter()
                    .map(|tool| prefixed_mcp_tool_name(server_id, &tool.tool_name))
                    .collect::<Vec<_>>();
                return Ok(ToolCatalogExecution {
                    output: json!({
                        "serverId": server_id,
                        "controlTool": control_tool,
                        "mounted": true,
                        "mountedToolCount": mounted_tools.len(),
                        "mountedTools": mounted_tools,
                        "status": "already_mounted",
                    }),
                    state_changes: Vec::new(),
                });
            }

            let raw_tools = mcp_manager
                .list_tools(server_id, server_config, mcp_runtime)
                .await?;
            let mounted_tools = super::schemas::normalize_mcp_tool_definitions(raw_tools);
            if mounted_tools.is_empty() {
                return Err(format!(
                    "MCP server '{}' did not expose any tools to mount",
                    server_id
                ));
            }

            let mounted_tool_names = mounted_tools
                .iter()
                .map(|tool| prefixed_mcp_tool_name(server_id, &tool.tool_name))
                .collect::<Vec<_>>();
            Ok(ToolCatalogExecution {
                output: json!({
                    "serverId": server_id,
                    "controlTool": control_tool,
                    "mounted": true,
                    "mountedToolCount": mounted_tool_names.len(),
                    "mountedTools": mounted_tool_names,
                    "status": "mounted",
                    "scope": "conversation",
                    "usage": {
                        "mount": { "action": "mount" },
                        "unmount": { "action": "unmount" },
                        "status": { "action": "status" }
                    }
                }),
                state_changes: vec![ToolCatalogStateChange::MountToolSource(
                    MountedToolSourceSession {
                        source_id: server_id.to_string(),
                        source_type: "mcp".to_string(),
                        tools: mounted_tools,
                        mcp_config: Some(server_config.clone()),
                    },
                )],
            })
        }
        "unmount" => Ok(ToolCatalogExecution {
            output: json!({
                "serverId": server_id,
                "controlTool": control_tool,
                "mounted": false,
                "status": if mounted_session.is_some() { "unmounted" } else { "already_unmounted" },
            }),
            state_changes: if mounted_session.is_some() {
                vec![ToolCatalogStateChange::UnmountToolSource {
                    source_id: server_id.to_string(),
                    source_type: "mcp".to_string(),
                }]
            } else {
                Vec::new()
            },
        }),
        _ => Err(format!(
            "Unsupported action '{}' for MCP server control tool '{}'. Use one of: mount, unmount, status.",
            action, control_tool
        )),
    }
}
