use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::fs;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::config::{self, AppConfig};

const MODEL_CACHE_FILE_NAME: &str = "models-cache.yaml";
pub const MODEL_CACHE_SYNC_INTERVAL_SECONDS: u64 = 30 * 60;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ModelCache {
  pub last_synced_unix: i64,
  pub models: Vec<String>,
  pub source_base_url: String,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteModelsResponse {
  data: Vec<RemoteModel>,
}

#[derive(Debug, Clone, Deserialize)]
struct RemoteModel {
  id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelCatalog {
  pub config_path: String,
  pub cache_path: String,
  pub default_model: String,
  pub configured_models: Vec<String>,
  pub cached_models: Vec<String>,
  pub effective_models: Vec<String>,
  pub cache_stale: bool,
  pub last_synced_unix: Option<i64>,
}

pub async fn get_model_catalog(sync_if_stale: bool) -> Result<ModelCatalog, String> {
  let config_path = config::init_config_if_missing()?;
  let cfg = config::load_config()?;
  let cache_path = model_cache_path()?;

  if sync_if_stale {
    let _ = sync_remote_model_cache_if_stale(&cfg).await;
  }

  let cache = load_model_cache().ok().flatten();
  let configured_models = dedup_models(cfg.available_models.clone());
  let cached_models = cache
    .as_ref()
    .map(|c| dedup_models(c.models.clone()))
    .unwrap_or_default();

  let mut effective_models = if configured_models.is_empty() {
    cached_models.clone()
  } else {
    configured_models.clone()
  };

  if !effective_models.iter().any(|m| m == &cfg.default_model) {
    effective_models.insert(0, cfg.default_model.clone());
  }

  Ok(ModelCatalog {
    config_path: config_path.display().to_string(),
    cache_path: cache_path.display().to_string(),
    default_model: cfg.default_model.clone(),
    configured_models,
    cached_models,
    effective_models,
    cache_stale: cache
      .as_ref()
      .map(cache_is_stale)
      .unwrap_or(true),
    last_synced_unix: cache.map(|c| c.last_synced_unix),
  })
}

pub async fn sync_remote_model_cache() -> Result<ModelCache, String> {
  let cfg = config::load_config()?;
  sync_remote_model_cache_with_config(&cfg).await
}

pub async fn sync_remote_model_cache_if_stale(cfg: &AppConfig) -> Result<Option<ModelCache>, String> {
  let cache = load_model_cache()?;
  let should_refresh = cache
    .as_ref()
    .map(cache_is_stale)
    .unwrap_or(true);

  if !should_refresh {
    return Ok(None);
  }

  let refreshed = sync_remote_model_cache_with_config(cfg).await?;
  Ok(Some(refreshed))
}

pub async fn sync_remote_model_cache_with_config(cfg: &AppConfig) -> Result<ModelCache, String> {
  let api_key = cfg
    .resolved_api_key()
    .ok_or_else(|| "OPENAI API key is missing. Cannot sync remote model cache.".to_string())?;

  let endpoint = format!("{}/models", cfg.base_url.trim_end_matches('/'));

  let response = reqwest::Client::new()
    .get(endpoint)
    .bearer_auth(api_key)
    .send()
    .await
    .map_err(|e| format!("Failed to fetch remote models: {e}"))?;

  if !response.status().is_success() {
    let status = response.status();
    let text = response
      .text()
      .await
      .unwrap_or_else(|_| "<unable to read error body>".to_string());
    return Err(format!("Failed to fetch remote models ({status}): {text}"));
  }

  let parsed: RemoteModelsResponse = response
    .json()
    .await
    .map_err(|e| format!("Failed to parse remote model list: {e}"))?;

  let mut ids = parsed.data.into_iter().map(|m| m.id).collect::<Vec<_>>();
  ids = dedup_models(ids);

  let cache = ModelCache {
    last_synced_unix: now_unix(),
    models: ids,
    source_base_url: cfg.base_url.clone(),
  };

  save_model_cache(&cache)?;
  Ok(cache)
}

fn model_cache_path() -> Result<std::path::PathBuf, String> {
  Ok(config::config_dir_path()?.join(MODEL_CACHE_FILE_NAME))
}

fn load_model_cache() -> Result<Option<ModelCache>, String> {
  let path = model_cache_path()?;
  if !path.exists() {
    return Ok(None);
  }

  let raw = fs::read_to_string(&path)
    .map_err(|e| format!("Failed to read model cache {}: {e}", path.display()))?;
  let parsed: ModelCache = serde_yaml::from_str(&raw)
    .map_err(|e| format!("Invalid YAML in model cache {}: {e}", path.display()))?;

  Ok(Some(parsed))
}

fn save_model_cache(cache: &ModelCache) -> Result<(), String> {
  let path = model_cache_path()?;
  let yaml = serde_yaml::to_string(cache)
    .map_err(|e| format!("Failed to serialize model cache: {e}"))?;

  fs::write(&path, yaml)
    .map_err(|e| format!("Failed to write model cache {}: {e}", path.display()))
}

fn cache_is_stale(cache: &ModelCache) -> bool {
  if cache.last_synced_unix <= 0 {
    return true;
  }
  let now = now_unix();
  now - cache.last_synced_unix >= MODEL_CACHE_SYNC_INTERVAL_SECONDS as i64
}

fn dedup_models(models: Vec<String>) -> Vec<String> {
  let mut set = BTreeSet::new();
  for model in models {
    let trimmed = model.trim();
    if !trimmed.is_empty() {
      set.insert(trimmed.to_string());
    }
  }
  set.into_iter().collect()
}

fn now_unix() -> i64 {
  SystemTime::now()
    .duration_since(UNIX_EPOCH)
    .map(|d| d.as_secs() as i64)
    .unwrap_or(0)
}
