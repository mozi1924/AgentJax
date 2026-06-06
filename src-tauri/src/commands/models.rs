use crate::error::AgentJaxError;
use crate::models;
use crate::models::ModelCatalog;

#[tauri::command]
pub async fn get_model_catalog() -> Result<ModelCatalog, AgentJaxError> {
    models::get_model_catalog(true).await
}

#[tauri::command]
pub async fn force_sync_model_cache() -> Result<ModelCatalog, AgentJaxError> {
    let _ = models::sync_remote_model_cache().await?;
    models::get_model_catalog(false).await
}
