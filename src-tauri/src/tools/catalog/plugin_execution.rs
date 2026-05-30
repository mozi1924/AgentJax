use super::types::ToolCatalogExecution;
use crate::plugin_runtime::{
    DenoCorePluginRuntime, PluginInvocationContext, PluginPackage, PluginRuntime, SandboxPolicy,
};
use crate::tools::ToolExecutionContext;
use serde_json::Value;

/// Execute a packaged plugin tool through an isolated deno_core runtime.
pub(super) fn execute_plugin_package_tool(
    package: &PluginPackage,
    plugin_id: &str,
    tool_name: &str,
    arguments: &Value,
    context: &ToolExecutionContext,
) -> Result<ToolCatalogExecution, String> {
    let mut plugin_runtime = DenoCorePluginRuntime::new(
        deno_core::RuntimeOptions::default(),
        SandboxPolicy::default(),
    );
    plugin_runtime
        .register_package(package.clone())
        .map_err(|err| err.to_string())?;
    let call = plugin_runtime
        .prepare_tool_call(
            plugin_id,
            tool_name,
            arguments.clone(),
            PluginInvocationContext {
                conversation_id: context.conversation_id.clone(),
            },
        )
        .map_err(|err| err.to_string())?;
    let result = plugin_runtime
        .execute_tool_call(call)
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
