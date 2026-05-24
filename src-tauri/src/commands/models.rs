use crate::models::ModelCatalog;
use crate::{config, models};

#[tauri::command]
pub async fn get_model_catalog() -> Result<ModelCatalog, String> {
    models::get_model_catalog(true).await
}

#[tauri::command]
pub async fn force_sync_model_cache() -> Result<ModelCatalog, String> {
    let cfg = config::load_config()?;
    let _ = models::sync_remote_model_cache_with_config(&cfg).await?;
    models::get_model_catalog(false).await
}
