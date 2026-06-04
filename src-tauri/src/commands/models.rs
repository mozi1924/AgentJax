use crate::models::ModelCatalog;
use crate::models;

#[tauri::command]
pub async fn get_model_catalog() -> Result<ModelCatalog, String> {
    models::get_model_catalog(true).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn force_sync_model_cache() -> Result<ModelCatalog, String> {
    let _ = models::sync_remote_model_cache().await?;
    models::get_model_catalog(false).await.map_err(|e| e.to_string())
}
