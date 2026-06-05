use super::types::ToolCatalogExecution;
use crate::agentjax_err;
use crate::error::{AgentJaxError, AgentJaxResult};
use crate::plugin_runtime::{PluginInvocationContext, PluginPackage, create_temp_plugin_instance};
use crate::tools::ToolExecutionContext;
use serde_json::Value;

/// Execute a packaged plugin tool through an isolated deno_core runtime.
///
/// Uses a temporary `PluginInstance` (fresh JsRuntime per call) rather than
/// the persistent `DenoCorePluginRuntime` because JsRuntime is not `Send` and
/// this function may be called from async contexts.
pub(super) fn execute_plugin_package_tool(
    package: &PluginPackage,
    plugin_id: &str,
    tool_name: &str,
    arguments: &Value,
    context: &ToolExecutionContext,
) -> AgentJaxResult<ToolCatalogExecution> {
    // Validate the tool exists in the manifest
    let manifest = &package.manifest;
    if !manifest.tools.iter().any(|t| t.name == tool_name) {
        return Err(agentjax_err!(
            format!(
                "Plugin '{}' does not export a tool named '{}'",
                plugin_id, tool_name
            ),
            ToolExecution
        ));
    }

    // Create a temporary PluginInstance, call the tool, then drop it
    let mut instance = create_temp_plugin_instance(package).map_err(|err| {
        AgentJaxError::tool(format!("Failed to create plugin instance: {err}"))
            .with_error_source(&err)
    })?;

    let result = instance
        .call_tool(
            tool_name,
            arguments.clone(),
            PluginInvocationContext {
                conversation_id: context.conversation_id.clone(),
                model_id: context.model_id.clone(),
                turn_id: context.turn_id.clone(),
                hop_index: context.hop_index,
                context_token_estimate: None,
                message_count: None,
                tool_call_count: None,
            },
        )
        .map_err(|err| AgentJaxError::tool(err.to_string()))?;

    if result.ok {
        Ok(ToolCatalogExecution {
            output: result.output,
            state_changes: Vec::new(),
        })
    } else {
        Err(agentjax_err!(
            result
                .error
                .unwrap_or_else(|| "Plugin tool execution failed".to_string()),
            ToolExecution
        ))
    }
}
