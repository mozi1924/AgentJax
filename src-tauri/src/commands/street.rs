//! Tauri IPC commands for Street notification management.

use crate::error::AgentJaxError;
use crate::street::{StreetManager, StreetSnapshot};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GetStreetItemsRequest {
    pub conversation_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DismissStreetItemRequest {
    pub item_id: String,
    pub conversation_id: String,
}

/// Get pending Street items for a conversation.
#[tauri::command]
pub fn get_street_items(req: GetStreetItemsRequest) -> Result<Vec<StreetSnapshot>, AgentJaxError> {
    Ok(StreetManager::get_pending_snapshots(&req.conversation_id))
}

/// Dismiss a single Street item.
#[tauri::command]
pub fn dismiss_street_item(req: DismissStreetItemRequest) -> Result<bool, AgentJaxError> {
    Ok(StreetManager::mark_dismissed(
        &req.item_id,
        &req.conversation_id,
    ))
}
