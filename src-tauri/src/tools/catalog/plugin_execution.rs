use super::types::ToolCatalogExecution;
use crate::plugin_runtime::{
    PluginInvocationContext, PluginPackage, create_temp_plugin_instance,
};
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
) -> Result<ToolCatalogExecution, String> {
    // Validate the tool exists in the manifest
    let manifest = &package.manifest;
    if !manifest.tools.iter().any(|t| t.name == tool_name) {
        return Err(format!(
            "Plugin '{}' does not export a tool named '{}'",
            plugin_id, tool_name
        ));
    }

    // Create a temporary PluginInstance, call the tool, then drop it
    let mut instance = create_temp_plugin_instance(package)
        .map_err(|err| format!("Failed to create plugin instance: {err}"))?;

    let result = instance
        .call_tool(
            tool_name,
            arguments.clone(),
            PluginInvocationContext {
                conversation_id: context.conversation_id.clone(),
            },
        )
        .map_err(|err| err.to_string())?;

    if result.ok {
        Ok(ToolCatalogExecution {
            output: result.output,
            state_changes: Vec::new(),
        })
    } else {
        Err(result
            .error
            .unwrap_or_else(|| "Plugin tool execution failed".to_string()))
    }
}
