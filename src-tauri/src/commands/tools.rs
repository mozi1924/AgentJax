use crate::config;
use crate::tools::{ToolCatalog, ToolManagerSnapshot, ToolManagerSnapshotRequest};
use std::sync::Arc;
use tauri::State;

#[tauri::command]
pub async fn get_tool_manager_snapshot(
    mcp_manager: State<'_, Arc<crate::mcp::McpManager>>,
    request: Option<ToolManagerSnapshotRequest>,
) -> Result<ToolManagerSnapshot, String> {
    let config = config::load_config()?;
    let catalog = ToolCatalog::new_with_home_plugins(mcp_manager.inner().clone(), &config);
    Ok(catalog
        .tool_manager_snapshot(request.unwrap_or_default())
        .await)
}
